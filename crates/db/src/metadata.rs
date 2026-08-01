use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::util::eq_predicate;
use open_course_core::error::Result;

pub const TABLE_NAME: &str = "metadata";

/// Well-known metadata keys.
pub const KEY_SCHEMA_VERSION: &str = "schema_version";
pub const KEY_LAST_PULLED_SEQ: &str = "last_pulled_seq";
pub const KEY_SYNC_ENABLED: &str = "sync_enabled";
pub const KEY_CLOUD_CURRICULUM_VERSION: &str = "cloud_curriculum_version";
pub const KEY_RESET_AT: &str = "reset_at";
pub const KEY_LAST_SYNC_AT: &str = "last_sync_at";

/// Service keys that survive `reset`: wiping them would lose the schema
/// version (triggering pointless re-migrations) and the sync state.
pub const PRESERVED_KEYS: &[&str] = &[
    KEY_SCHEMA_VERSION,
    KEY_LAST_PULLED_SEQ,
    KEY_SYNC_ENABLED,
    KEY_CLOUD_CURRICULUM_VERSION,
];

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
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("key", key))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
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
        self.table
            .delete(&eq_predicate("key", key))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(StringArray::from(vec![key])),
                Arc::new(StringArray::from(vec![value])),
            ],
        )
        .map_err(crate::error::DbError::from)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
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

    /// An optional i32: `None` when the key is missing or unparsable.
    pub async fn get_optional_i32(&self, key: &str) -> Result<Option<i32>> {
        match self.get(key).await? {
            Some(value) => Ok(value.parse::<i32>().ok()),
            None => Ok(None),
        }
    }

    pub async fn get_i64(&self, key: &str) -> Result<i64> {
        match self.get(key).await? {
            Some(value) => Ok(value.parse::<i64>().unwrap_or(0)),
            None => Ok(0),
        }
    }

    pub async fn set_i64(&self, key: &str, value: i64) -> Result<()> {
        self.set(key, &value.to_string()).await
    }

    /// A boolean stored as the string "true"/"false"; missing key is false.
    pub async fn get_bool(&self, key: &str) -> Result<bool> {
        Ok(self.get(key).await?.as_deref() == Some("true"))
    }

    pub async fn set_bool(&self, key: &str, value: bool) -> Result<()> {
        self.set(key, if value { "true" } else { "false" }).await
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

    /// Wipes user keys (session counters, alerts, ...) but preserves the
    /// service keys in `PRESERVED_KEYS` (schema version, sync state).
    pub async fn reset(&self) -> Result<()> {
        let preserved: Vec<(String, String)> = {
            let mut out = Vec::new();
            for key in PRESERVED_KEYS {
                if let Some(value) = self.get(key).await? {
                    out.push((key.to_string(), value));
                }
            }
            out
        };
        self.table
            .delete("key IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        for (key, value) in preserved {
            self.set(&key, &value).await?;
        }
        Ok(())
    }

    /// Local schema version of this pair's tables; 1 when never stamped.
    pub async fn schema_version(&self) -> Result<i32> {
        match self.get(KEY_SCHEMA_VERSION).await? {
            Some(value) => Ok(value.parse::<i32>().unwrap_or(1)),
            None => Ok(1),
        }
    }

    pub async fn set_schema_version(&self, version: i32) -> Result<()> {
        self.set_i32(KEY_SCHEMA_VERSION, version).await
    }

    /// Outbox sequence the sync engine has pulled up to; 0 when never synced.
    pub async fn last_pulled_seq(&self) -> Result<i64> {
        self.get_i64(KEY_LAST_PULLED_SEQ).await
    }

    pub async fn set_last_pulled_seq(&self, seq: i64) -> Result<()> {
        self.set_i64(KEY_LAST_PULLED_SEQ, seq).await
    }

    pub async fn sync_enabled(&self) -> Result<bool> {
        self.get_bool(KEY_SYNC_ENABLED).await
    }

    pub async fn set_sync_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(KEY_SYNC_ENABLED, enabled).await
    }

    pub async fn cloud_curriculum_version(&self) -> Result<Option<i32>> {
        self.get_optional_i32(KEY_CLOUD_CURRICULUM_VERSION).await
    }

    pub async fn set_cloud_curriculum_version(&self, version: i32) -> Result<()> {
        self.set_i32(KEY_CLOUD_CURRICULUM_VERSION, version).await
    }

    /// RFC3339 time of the last successful sync (pull or push), if any.
    pub async fn last_sync_at(&self) -> Result<Option<String>> {
        self.get(KEY_LAST_SYNC_AT).await
    }

    pub async fn set_last_sync_at(&self, at: &str) -> Result<()> {
        self.set(KEY_LAST_SYNC_AT, at).await
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::Database;

    #[tokio::test]
    async fn progress_reset_preserves_service_keys() {
        let dir = TempDir::new().unwrap();
        let db = Database::connect(&dir.path().join("db")).await.unwrap();
        let metadata = db.metadata();

        let schema_version = metadata.schema_version().await.unwrap();
        metadata.set_last_pulled_seq(42).await.unwrap();
        metadata.set_sync_enabled(true).await.unwrap();
        metadata.set_cloud_curriculum_version(3).await.unwrap();
        metadata.set_i32("session_count", 7).await.unwrap();

        db.progress().reset().await.unwrap();

        assert_eq!(metadata.schema_version().await.unwrap(), schema_version);
        assert_eq!(metadata.last_pulled_seq().await.unwrap(), 42);
        assert!(metadata.sync_enabled().await.unwrap());
        assert_eq!(metadata.cloud_curriculum_version().await.unwrap(), Some(3));
        // User keys are still wiped.
        assert_eq!(metadata.get_i32("session_count").await.unwrap(), 0);
    }
}
