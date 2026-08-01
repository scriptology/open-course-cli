use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::metadata::MetadataTable;
use crate::util::eq_predicate;

use open_course_core::error::Result;
pub use open_course_core::progress::*;

pub const TABLE_NAME: &str = "progress";

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("topic_id", DataType::Utf8, false),
        Field::new("score", DataType::Float64, false),
        Field::new("mastery", DataType::Float64, false),
        Field::new("difficulty_estimate", DataType::Float64, false),
        Field::new("practice_count", DataType::Int32, false),
        Field::new("last_practiced", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, true),
        Field::new("deleted_at", DataType::Utf8, true),
    ]))
}

async fn open_or_migrate_progress_table(connection: &Connection) -> Result<lancedb::Table> {
    let names = connection.table_names().execute().await.unwrap_or_default();
    if !names.contains(&TABLE_NAME.to_string()) {
        return connection
            .create_empty_table(TABLE_NAME, schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)
            .map_err(Into::into);
    }

    let existing = connection
        .open_table(TABLE_NAME)
        .execute()
        .await
        .map_err(crate::error::DbError::from)?;
    let existing_schema = existing
        .schema()
        .await
        .map_err(crate::error::DbError::from)?;
    if schema_compatible(&existing_schema, &schema()) {
        Ok(existing)
    } else {
        migrate_progress_table(connection, existing).await
    }
}

fn schema_compatible(existing: &Arc<Schema>, target: &Arc<Schema>) -> bool {
    if existing.fields().len() != target.fields().len() {
        return false;
    }
    existing
        .fields()
        .iter()
        .zip(target.fields().iter())
        .all(|(a, b)| a.name() == b.name() && a.data_type() == b.data_type())
}

async fn migrate_progress_table(
    connection: &Connection,
    old_table: lancedb::Table,
) -> Result<lancedb::Table> {
    let records: Vec<RecordBatch> = old_table
        .query()
        .execute()
        .await
        .map_err(crate::error::DbError::from)?
        .try_collect()
        .await
        .map_err(crate::error::DbError::from)?;
    let mut topics = Vec::new();
    for batch in &records {
        topics.extend(progress_from_record_batch(batch)?.topics);
    }

    connection
        .drop_table(TABLE_NAME, &[])
        .await
        .map_err(crate::error::DbError::from)?;
    let new_table = connection
        .create_empty_table(TABLE_NAME, schema())
        .execute()
        .await
        .map_err(crate::error::DbError::from)?;
    if !topics.is_empty() {
        let batches = topics
            .iter()
            .map(progress_topic_to_record_batch)
            .collect::<Result<Vec<_>>>()?;
        new_table
            .add(batches)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    Ok(new_table)
}

#[derive(Clone)]
pub struct ProgressTable {
    table: lancedb::Table,
    metadata: MetadataTable,
}

impl ProgressTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let metadata = MetadataTable::open(connection).await?;
        Self::open_with_metadata(connection, metadata).await
    }

    /// Opens the progress table sharing an already-open metadata handle, so
    /// all metadata reads/writes in the process go through a single LanceDB
    /// table handle (separate handles do not see each other's commits).
    pub(crate) async fn open_with_metadata(
        connection: &Connection,
        metadata: MetadataTable,
    ) -> Result<Self> {
        let table = open_or_migrate_progress_table(connection).await?;
        Ok(Self { table, metadata })
    }

    /// Reads all non-deleted progress entries.
    pub async fn read_all(&self) -> Result<ProgressData> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut data = if records.is_empty() {
            ProgressData {
                version: 3,
                topics: Vec::new(),
                ..Default::default()
            }
        } else {
            let mut all_topics = Vec::new();
            for batch in &records {
                all_topics.extend(progress_from_record_batch(batch)?.topics);
            }
            all_topics.retain(|t| t.deleted_at.is_none());
            ProgressData {
                version: 3,
                topics: all_topics,
                ..Default::default()
            }
        };
        data.session_count = self.metadata.get_i32("session_count").await?;
        data.adaptive_alerts = self.metadata.get_string_list("adaptive_alerts").await?;
        Ok(data)
    }

    pub async fn get_by_topic_id(&self, topic_id: &str) -> Result<Option<ProgressTopic>> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("topic_id", topic_id))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        if records.is_empty() {
            return Ok(None);
        }
        let mut all_topics = Vec::new();
        for batch in &records {
            all_topics.extend(progress_from_record_batch(batch)?.topics);
        }
        Ok(all_topics.into_iter().find(|t| t.deleted_at.is_none()))
    }

    /// Insert or replace an entry, stamping `updated_at` with the current time.
    pub async fn upsert(&self, topic: &ProgressTopic) -> Result<()> {
        let mut topic = topic.clone();
        topic.updated_at = Some(crate::util::now_rfc3339());
        self.upsert_with_timestamps(&topic).await
    }

    /// Insert or replace an entry exactly as given — used when applying
    /// synced changes whose timestamps must be preserved.
    pub async fn upsert_with_timestamps(&self, topic: &ProgressTopic) -> Result<()> {
        self.table
            .delete(&eq_predicate("topic_id", &topic.topic_id))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = progress_topic_to_record_batch(topic)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    pub async fn write_all(&self, data: &ProgressData) -> Result<()> {
        let mut data = data.clone();
        let now = crate::util::now_rfc3339();
        for topic in &mut data.topics {
            topic.updated_at = Some(now.clone());
        }
        self.write_all_with_timestamps(&data).await
    }

    /// Rewrite the whole table exactly as given — used when applying synced
    /// changes whose timestamps must be preserved.
    pub async fn write_all_with_timestamps(&self, data: &ProgressData) -> Result<()> {
        self.table
            .delete("topic_id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        if !data.topics.is_empty() {
            let mut batches = Vec::new();
            for topic in &data.topics {
                batches.push(progress_topic_to_record_batch(topic)?);
            }
            self.table
                .add(batches)
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
        self.metadata
            .set_i32("session_count", data.session_count)
            .await?;
        self.metadata
            .set_string_list("adaptive_alerts", &data.adaptive_alerts)
            .await?;
        Ok(())
    }

    /// Soft-delete: the row stays as a tombstone so sync can propagate the
    /// deletion; reads filter it out.
    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<()> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("topic_id", topic_id))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let now = crate::util::now_rfc3339();
        let mut tombstoned = Vec::new();
        for batch in &records {
            for mut topic in progress_from_record_batch(batch)?.topics {
                topic.deleted_at = Some(now.clone());
                tombstoned.push(topic);
            }
        }
        if tombstoned.is_empty() {
            return Ok(());
        }
        self.table
            .delete(&eq_predicate("topic_id", topic_id))
            .await
            .map_err(crate::error::DbError::from)?;
        for topic in &tombstoned {
            let batch = progress_topic_to_record_batch(topic)?;
            self.table
                .add(vec![batch])
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
        Ok(())
    }

    /// Physically removes tombstones older than `older_than` (RFC3339).
    /// Not called automatically.
    pub async fn purge_deleted(&self, older_than: &str) -> Result<()> {
        self.table
            .delete(&format!(
                "deleted_at IS NOT NULL AND deleted_at < '{}'",
                crate::util::sql_escape(older_than)
            ))
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table
            .delete("topic_id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        self.metadata.reset().await?;
        Ok(())
    }
}

pub(crate) fn progress_topic_to_record_batch(topic: &ProgressTopic) -> Result<RecordBatch> {
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![topic.topic_id.as_str()])),
            Arc::new(Float64Array::from(vec![topic.score])),
            Arc::new(Float64Array::from(vec![topic.mastery])),
            Arc::new(Float64Array::from(vec![topic.difficulty_estimate])),
            Arc::new(Int32Array::from(vec![topic.practice_count])),
            Arc::new(StringArray::from(vec![topic.last_practiced.as_deref()])),
            Arc::new(StringArray::from(vec![topic.updated_at.as_deref()])),
            Arc::new(StringArray::from(vec![topic.deleted_at.as_deref()])),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn progress_from_record_batch(batch: &RecordBatch) -> Result<ProgressData> {
    let n = batch.num_rows();
    let topic_id_col = batch
        .column_by_name("topic_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let score_col = batch
        .column_by_name("score")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let mastery_col = batch
        .column_by_name("mastery")
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
    let difficulty_col = batch
        .column_by_name("difficulty_estimate")
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
    let count_col = batch
        .column_by_name("practice_count")
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
    let last_col = batch
        .column_by_name("last_practiced")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    // Columns added in schema v2 may be absent in unmigrated tables.
    let updated_col = crate::util::optional_string_column(batch, "updated_at");
    let deleted_col = crate::util::optional_string_column(batch, "deleted_at");

    let mut topics = Vec::with_capacity(n);
    for i in 0..n {
        let score = score_col.value(i);
        let mastery = mastery_col.map(|c| c.value(i)).unwrap_or(score);
        topics.push(ProgressTopic {
            topic_id: topic_id_col.value(i).to_string(),
            score,
            mastery,
            difficulty_estimate: difficulty_col.map(|c| c.value(i)).unwrap_or(0.0),
            practice_count: count_col.map(|c| c.value(i)).unwrap_or(0),
            last_practiced: if last_col.is_null(i) {
                None
            } else {
                Some(last_col.value(i).to_string())
            },
            updated_at: crate::util::optional_string_at(updated_col, i),
            deleted_at: crate::util::optional_string_at(deleted_col, i),
        });
    }

    Ok(ProgressData {
        version: 3,
        topics,
        ..Default::default()
    })
}
