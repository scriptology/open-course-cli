use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "topic_reviews";

#[derive(Debug, Clone, PartialEq)]
pub struct TopicReview {
    pub topic_id: String,
    pub content: String,
    pub generated_at: String,
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("topic_id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("generated_at", DataType::Utf8, false),
    ]))
}

#[derive(Clone)]
pub struct ReviewsTable {
    table: lancedb::Table,
}

impl ReviewsTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await?;
        Ok(Self { table })
    }

    pub async fn get_by_topic_id(&self, topic_id: &str) -> Result<Option<TopicReview>> {
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
        let mut reviews = Vec::new();
        for batch in &records {
            reviews.extend(review_from_record_batch(batch)?);
        }
        Ok(reviews.into_iter().next())
    }

    pub async fn upsert(&self, review: &TopicReview) -> Result<()> {
        self.table
            .delete(&eq_predicate("topic_id", &review.topic_id))
            .await?;
        let batch = review_to_record_batch(review)?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn remove_by_topic_id(&self, topic_id: &str) -> Result<()> {
        self.table
            .delete(&eq_predicate("topic_id", topic_id))
            .await?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table.delete("topic_id IS NOT NULL").await?;
        Ok(())
    }
}

fn review_to_record_batch(review: &TopicReview) -> Result<RecordBatch> {
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![review.topic_id.as_str()])),
            Arc::new(StringArray::from(vec![review.content.as_str()])),
            Arc::new(StringArray::from(vec![review.generated_at.as_str()])),
        ],
    )?;
    Ok(batch)
}

fn review_from_record_batch(batch: &RecordBatch) -> Result<Vec<TopicReview>> {
    let n = batch.num_rows();
    let topic_id_col = batch
        .column_by_name("topic_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let content_col = batch
        .column_by_name("content")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let generated_at_col = batch
        .column_by_name("generated_at")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();

    let mut reviews = Vec::with_capacity(n);
    for i in 0..n {
        reviews.push(TopicReview {
            topic_id: topic_id_col.value(i).to_string(),
            content: content_col.value(i).to_string(),
            generated_at: generated_at_col.value(i).to_string(),
        });
    }
    Ok(reviews)
}
