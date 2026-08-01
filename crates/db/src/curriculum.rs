use std::sync::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{Array, Int32Array, ListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::util::eq_predicate;
use open_course_core::error::Result;

pub use open_course_core::curriculum::*;

pub const TABLE_NAME: &str = "curriculum";

/// Removes abstract/spelling topics from curriculum, progress, and reviews tables,
/// and moves micro-topics (concrete learning items such as "X vs Y" or "Rule: example")
/// into the `learning_items` table while preserving their scores.
pub async fn cleanup_topics(db: &crate::Database) -> Result<(usize, usize)> {
    use crate::learning_items::{LearningItem, is_learning_item_name};

    let curriculum = db.curriculum().read_all().await?;
    let progress_data = db.progress().read_all().await?;
    let progress_by_id: std::collections::HashMap<String, crate::progress::ProgressTopic> =
        progress_data
            .topics
            .into_iter()
            .map(|t| (t.topic_id.clone(), t))
            .collect();

    let mut moved = 0usize;
    let mut removed = 0usize;

    for topic in &curriculum.topics {
        let name = topic.name.trim();
        let is_micro = is_learning_item_name(name)
            && (name.contains(':') || name.to_lowercase().contains(" vs ") || name.contains('/'));
        let is_bad = should_remove_topic(name);

        if is_micro {
            let mut item = LearningItem::from_topic(topic);
            if let Some(p) = progress_by_id.get(&topic.id) {
                item.score = p.score;
                item.last_practiced = p.last_practiced.clone();
                item.practice_count = p.practice_count;
            }
            db.learning_items().upsert(&item).await?;
            let _ = db.curriculum().delete_by_topic_id(&topic.id).await;
            let _ = db.progress().delete_by_topic_id(&topic.id).await;
            let _ = db.reviews().remove_by_topic_id(&topic.id).await;
            moved += 1;
        } else if is_bad {
            let _ = db.curriculum().delete_by_topic_id(&topic.id).await;
            let _ = db.progress().delete_by_topic_id(&topic.id).await;
            let _ = db.reviews().remove_by_topic_id(&topic.id).await;
            let _ = db.learning_items().delete_by_id(&topic.id).await;
            removed += 1;
        }
    }

    Ok((moved, removed))
}

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, false),
        Field::new("difficulty", DataType::Utf8, false),
        Field::new("level", DataType::Utf8, true),
        Field::new("order", DataType::Int32, true),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("target_lang", DataType::Utf8, false),
        Field::new("native_lang", DataType::Utf8, false),
        Field::new("version", DataType::Int32, false),
        Field::new("updated_at", DataType::Utf8, true),
        Field::new("deleted_at", DataType::Utf8, true),
    ]))
}

#[derive(Clone)]
pub struct CurriculumTable {
    table: lancedb::Table,
}

impl CurriculumTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    /// Reads all non-deleted topics.
    pub async fn read_all(&self) -> Result<Curriculum> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        if records.is_empty() {
            return Ok(Curriculum {
                version: 1,
                topics: Vec::new(),
                target_language: String::new(),
                native_language: String::new(),
            });
        }

        let mut all_topics = Vec::new();
        let mut version = 1;
        let mut target_language = String::new();
        let mut native_language = String::new();
        for batch in &records {
            let parsed = topics_from_record_batch(batch)?;
            if !parsed.topics.is_empty() {
                version = parsed.version;
                target_language = parsed.target_language;
                native_language = parsed.native_language;
            }
            all_topics.extend(parsed.topics);
        }
        all_topics.retain(|t| t.deleted_at.is_none());

        sort_topics(&mut all_topics);

        Ok(Curriculum {
            version,
            topics: all_topics,
            target_language,
            native_language,
        })
    }

    /// Insert or replace a topic, stamping `updated_at` with the current time.
    pub async fn upsert(&self, topic: &Topic) -> Result<()> {
        let mut topic = topic.clone();
        topic.updated_at = Some(crate::util::now_rfc3339());
        self.upsert_with_timestamps(&topic).await
    }

    /// Insert or replace a topic exactly as given — used when applying
    /// synced changes whose timestamps must be preserved.
    pub async fn upsert_with_timestamps(&self, topic: &Topic) -> Result<()> {
        self.table
            .delete(&eq_predicate("id", &topic.id))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = topic_to_record_batch(topic)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    /// Soft-delete: the row stays as a tombstone so sync can propagate the
    /// deletion; reads filter it out.
    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<()> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("id", topic_id))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let now = crate::util::now_rfc3339();
        let mut tombstoned = Vec::new();
        for batch in &records {
            for mut topic in topics_from_record_batch(batch)?.topics {
                topic.deleted_at = Some(now.clone());
                tombstoned.push(topic);
            }
        }
        if tombstoned.is_empty() {
            return Ok(());
        }
        self.table
            .delete(&eq_predicate("id", topic_id))
            .await
            .map_err(crate::error::DbError::from)?;
        for topic in &tombstoned {
            let batch = topic_to_record_batch(topic)?;
            self.table
                .add(vec![batch])
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
        Ok(())
    }

    /// Physically removes tombstones older than `older_than` (RFC3339).
    /// Not called automatically — sync garbage collection invokes it once
    /// deletions have propagated.
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

    pub async fn delete_all(&self) -> Result<()> {
        self.table
            .delete("id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.delete_all().await?;
        Ok(())
    }
}

fn sort_topics(topics: &mut [Topic]) {
    topics.sort_by(|a, b| {
        let order_a = a.order.unwrap_or(i32::MAX);
        let order_b = b.order.unwrap_or(i32::MAX);
        match order_a.cmp(&order_b) {
            std::cmp::Ordering::Equal => a.cefr_numeric().cmp(&b.cefr_numeric()),
            other => other,
        }
    });
}

pub(crate) fn topic_to_record_batch(topic: &Topic) -> Result<RecordBatch> {
    let mut tags_builder = ListBuilder::new(StringBuilder::new());
    for tag in &topic.tags {
        tags_builder.values().append_value(tag);
    }
    tags_builder.append(true);
    let tags_array = tags_builder.finish();

    let level_value = topic.level.as_deref().unwrap_or("");
    let order_value = topic.order.unwrap_or(0);

    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![topic.id.as_str()])),
            Arc::new(StringArray::from(vec![topic.name.as_str()])),
            Arc::new(StringArray::from(vec![topic.description.as_str()])),
            Arc::new(StringArray::from(vec![topic.difficulty.as_str()])),
            Arc::new(StringArray::from(vec![level_value])),
            Arc::new(Int32Array::from(vec![order_value])),
            Arc::new(tags_array),
            Arc::new(StringArray::from(vec![topic.target_lang.as_str()])),
            Arc::new(StringArray::from(vec![topic.native_lang.as_str()])),
            Arc::new(Int32Array::from(vec![topic.version])),
            Arc::new(StringArray::from(vec![topic.updated_at.as_deref()])),
            Arc::new(StringArray::from(vec![topic.deleted_at.as_deref()])),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn topics_from_record_batch(batch: &RecordBatch) -> Result<Curriculum> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Curriculum {
            version: 1,
            topics: Vec::new(),
            target_language: String::new(),
            native_language: String::new(),
        });
    }

    let id_col = batch
        .column_by_name("id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let name_col = batch
        .column_by_name("name")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let desc_col = batch
        .column_by_name("description")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let diff_col = batch
        .column_by_name("difficulty")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let level_col = batch
        .column_by_name("level")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let order_col = batch
        .column_by_name("order")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    let tags_col = batch
        .column_by_name("tags")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    let target_col = batch
        .column_by_name("target_lang")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let native_col = batch
        .column_by_name("native_lang")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let version_col = batch
        .column_by_name("version")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    // Columns added in schema v2 may be absent in unmigrated tables.
    let updated_col = crate::util::optional_string_column(batch, "updated_at");
    let deleted_col = crate::util::optional_string_column(batch, "deleted_at");

    let mut topics = Vec::with_capacity(n);
    for i in 0..n {
        let tags_list = tags_col.value(i);
        let tags = tags_list
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .iter()
            .filter_map(|s| s.map(|s| s.to_string()))
            .collect();
        let level = if level_col.is_null(i) || level_col.value(i).is_empty() {
            None
        } else {
            Some(level_col.value(i).to_string())
        };
        let order = if order_col.is_null(i) || order_col.value(i) == 0 {
            None
        } else {
            Some(order_col.value(i))
        };
        topics.push(Topic {
            id: id_col.value(i).to_string(),
            name: name_col.value(i).to_string(),
            description: desc_col.value(i).to_string(),
            difficulty: diff_col.value(i).to_string(),
            level,
            order,
            tags,
            target_lang: target_col.value(i).to_string(),
            native_lang: native_col.value(i).to_string(),
            version: version_col.value(i),
            updated_at: crate::util::optional_string_at(updated_col, i),
            deleted_at: crate::util::optional_string_at(deleted_col, i),
        });
    }

    Ok(Curriculum {
        version: version_col.value(0),
        topics,
        target_language: target_col.value(0).to_string(),
        native_language: native_col.value(0).to_string(),
    })
}
