use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::db::metadata::MetadataTable;
use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "progress";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProgressTopic {
    pub topic_id: String,
    pub score: f64,
    pub mastery: f64,
    pub difficulty_estimate: f64,
    pub practice_count: i32,
    pub last_practiced: Option<String>,
}

impl ProgressTopic {
    /// A fresh, never-practiced progress entry starting at `initial_score`.
    pub fn initial(topic_id: String, initial_score: f64) -> Self {
        Self {
            topic_id,
            score: initial_score,
            mastery: initial_score,
            difficulty_estimate: 0.0,
            practice_count: 0,
            last_practiced: None,
        }
    }
}

/// Starting score for a newly added topic: material below the user's CEFR
/// level is treated as already familiar (100), everything else starts at 0.
pub fn initial_topic_score(topic_cefr: i32, user_cefr: i32) -> f64 {
    if topic_cefr > 0 && topic_cefr < user_cefr {
        100.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProgressData {
    pub version: i32,
    pub topics: Vec<ProgressTopic>,
    pub session_count: i32,
    pub adaptive_alerts: Vec<String>,
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("topic_id", DataType::Utf8, false),
        Field::new("score", DataType::Float64, false),
        Field::new("mastery", DataType::Float64, false),
        Field::new("difficulty_estimate", DataType::Float64, false),
        Field::new("practice_count", DataType::Int32, false),
        Field::new("last_practiced", DataType::Utf8, true),
    ]))
}

async fn open_or_migrate_progress_table(connection: &Connection) -> Result<lancedb::Table> {
    let names = connection.table_names().execute().await.unwrap_or_default();
    if !names.contains(&TABLE_NAME.to_string()) {
        return connection
            .create_empty_table(TABLE_NAME, schema())
            .execute()
            .await
            .map_err(Into::into);
    }

    let existing = connection.open_table(TABLE_NAME).execute().await?;
    let existing_schema = existing.schema().await?;
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
    let records: Vec<RecordBatch> = old_table.query().execute().await?.try_collect().await?;
    let mut topics = Vec::new();
    for batch in &records {
        topics.extend(progress_from_record_batch(batch)?.topics);
    }

    connection.drop_table(TABLE_NAME, &[]).await?;
    let new_table = connection
        .create_empty_table(TABLE_NAME, schema())
        .execute()
        .await?;
    if !topics.is_empty() {
        let batches = topics
            .iter()
            .map(progress_topic_to_record_batch)
            .collect::<Result<Vec<_>>>()?;
        new_table.add(batches).execute().await?;
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
        let table = open_or_migrate_progress_table(connection).await?;
        let metadata = MetadataTable::open(connection).await?;
        Ok(Self { table, metadata })
    }

    pub async fn read_all(&self) -> Result<ProgressData> {
        let records = self
            .table
            .query()
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
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
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        if records.is_empty() {
            return Ok(None);
        }
        let mut all_topics = Vec::new();
        for batch in &records {
            all_topics.extend(progress_from_record_batch(batch)?.topics);
        }
        Ok(all_topics.into_iter().next())
    }

    pub async fn upsert(&self, topic: &ProgressTopic) -> Result<()> {
        self.table
            .delete(&eq_predicate("topic_id", &topic.topic_id))
            .await?;
        let batch = progress_topic_to_record_batch(topic)?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn write_all(&self, data: &ProgressData) -> Result<()> {
        self.table.delete("topic_id IS NOT NULL").await?;
        if !data.topics.is_empty() {
            let mut batches = Vec::new();
            for topic in &data.topics {
                batches.push(progress_topic_to_record_batch(topic)?);
            }
            self.table.add(batches).execute().await?;
        }
        self.metadata
            .set_i32("session_count", data.session_count)
            .await?;
        self.metadata
            .set_string_list("adaptive_alerts", &data.adaptive_alerts)
            .await?;
        Ok(())
    }

    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<()> {
        self.table
            .delete(&eq_predicate("topic_id", topic_id))
            .await?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table.delete("topic_id IS NOT NULL").await?;
        self.metadata.reset().await?;
        Ok(())
    }
}

fn progress_topic_to_record_batch(topic: &ProgressTopic) -> Result<RecordBatch> {
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![topic.topic_id.as_str()])),
            Arc::new(Float64Array::from(vec![topic.score])),
            Arc::new(Float64Array::from(vec![topic.mastery])),
            Arc::new(Float64Array::from(vec![topic.difficulty_estimate])),
            Arc::new(Int32Array::from(vec![topic.practice_count])),
            Arc::new(StringArray::from(vec![topic.last_practiced.as_deref()])),
        ],
    )?;
    Ok(batch)
}

fn progress_from_record_batch(batch: &RecordBatch) -> Result<ProgressData> {
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
        });
    }

    Ok(ProgressData {
        version: 3,
        topics,
        ..Default::default()
    })
}
