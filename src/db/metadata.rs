use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "metadata";

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ]))
}

#[derive(Clone)]
pub struct MetadataTable {
    table: lancedb::Table,
}

impl MetadataTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await?;
        Ok(Self { table })
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("key", key))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        if records.is_empty() {
            return Ok(None);
        }
        let value_col = records[0]
            .column_by_name("value")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        if value_col.is_null(0) {
            Ok(None)
        } else {
            Ok(Some(value_col.value(0).to_string()))
        }
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<()> {
        self.table.delete(&eq_predicate("key", key)).await?;
        let batch = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(StringArray::from(vec![key])),
                Arc::new(StringArray::from(vec![value])),
            ],
        )?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn get_i32(&self, key: &str) -> Result<i32> {
        match self.get(key).await? {
            Some(value) => Ok(value.parse::<i32>().unwrap_or(0)),
            None => Ok(0),
        }
    }

    pub async fn set_i32(&self, key: &str, value: i32) -> Result<()> {
        self.set(key, &value.to_string()).await
    }

    pub async fn get_string_list(&self, key: &str) -> Result<Vec<String>> {
        match self.get(key).await? {
            Some(value) if !value.is_empty() => {
                Ok(serde_json::from_str(&value).unwrap_or_default())
            }
            _ => Ok(Vec::new()),
        }
    }

    pub async fn set_string_list(&self, key: &str, value: &[String]) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.set(key, &json).await
    }

    pub async fn reset(&self) -> Result<()> {
        self.table.delete("key IS NOT NULL").await?;
        Ok(())
    }
}
