use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::db::metadata::MetadataTable;
use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "progress";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProgressTopic {
    pub topic_id: String,
    pub score: f64,
    pub last_practiced: Option<String>,
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
        Field::new("last_practiced", DataType::Utf8, true),
    ]))
}

#[derive(Clone)]
pub struct ProgressTable {
    table: lancedb::Table,
    metadata: MetadataTable,
}

impl ProgressTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await?;
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
                version: 2,
                topics: Vec::new(),
                ..Default::default()
            }
        } else {
            let mut all_topics = Vec::new();
            for batch in &records {
                all_topics.extend(progress_from_record_batch(batch)?.topics);
            }
            ProgressData {
                version: 2,
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
    let last_col = batch
        .column_by_name("last_practiced")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();

    let mut topics = Vec::with_capacity(n);
    for i in 0..n {
        topics.push(ProgressTopic {
            topic_id: topic_id_col.value(i).to_string(),
            score: score_col.value(i),
            last_practiced: if last_col.is_null(i) {
                None
            } else {
                Some(last_col.value(i).to_string())
            },
        });
    }

    Ok(ProgressData {
        version: 2,
        topics,
        ..Default::default()
    })
}
