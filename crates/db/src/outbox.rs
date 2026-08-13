//! Sync outbox: a local, monotonically ordered log of mutations to synced
//! entities. The future sync engine drains it with `read_all` +
//! `delete_through`; nothing reads it automatically yet.

use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::ExecutableQuery;

use open_course_core::error::Result;

pub const TABLE_NAME: &str = "outbox";

pub const OP_UPSERT: &str = "upsert";
pub const OP_DELETE: &str = "delete";
pub const OP_TOMBSTONE_RESET: &str = "tombstone_reset";

pub const ENTITY_TOPIC: &str = "topic";
pub const ENTITY_PROGRESS: &str = "progress";
pub const ENTITY_SESSION: &str = "session";
pub const ENTITY_LEARNING_ITEM: &str = "learning_item";
pub const ENTITY_LEMMA: &str = "lemma";
pub const ENTITY_FORM: &str = "form";
pub const ENTITY_METADATA: &str = "metadata";

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEntry {
    pub seq: i64,
    pub op: String,
    pub entity: String,
    pub entity_id: String,
    pub payload: String,
    pub created_at: String,
}

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("seq", DataType::Int64, false),
        Field::new("op", DataType::Utf8, false),
        Field::new("entity", DataType::Utf8, false),
        Field::new("entity_id", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
    ]))
}

#[derive(Clone)]
pub struct OutboxTable {
    table: lancedb::Table,
}

impl OutboxTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    /// Appends an entry with the next local sequence number.
    pub async fn append(
        &self,
        op: &str,
        entity: &str,
        entity_id: &str,
        payload: &str,
    ) -> Result<OutboxEntry> {
        let entry = OutboxEntry {
            seq: self.next_seq().await?,
            op: op.to_string(),
            entity: entity.to_string(),
            entity_id: entity_id.to_string(),
            payload: payload.to_string(),
            created_at: crate::util::now_rfc3339(),
        };
        let batch = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(Int64Array::from(vec![entry.seq])),
                Arc::new(StringArray::from(vec![entry.op.as_str()])),
                Arc::new(StringArray::from(vec![entry.entity.as_str()])),
                Arc::new(StringArray::from(vec![entry.entity_id.as_str()])),
                Arc::new(StringArray::from(vec![entry.payload.as_str()])),
                Arc::new(StringArray::from(vec![entry.created_at.as_str()])),
            ],
        )
        .map_err(crate::error::DbError::from)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(entry)
    }

    async fn next_seq(&self) -> Result<i64> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut max_seq = 0i64;
        for batch in &records {
            let Some(seq_col) = batch
                .column_by_name("seq")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            else {
                continue;
            };
            for i in 0..batch.num_rows() {
                max_seq = max_seq.max(seq_col.value(i));
            }
        }
        Ok(max_seq + 1)
    }

    /// All entries ordered by `seq`.
    pub async fn read_all(&self) -> Result<Vec<OutboxEntry>> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut entries = Vec::new();
        for batch in &records {
            entries.extend(entries_from_record_batch(batch)?);
        }
        entries.sort_by_key(|e| e.seq);
        Ok(entries)
    }

    /// Removes entries up to and including `seq` (after a successful push).
    pub async fn delete_through(&self, seq: i64) -> Result<()> {
        self.table
            .delete(&format!("seq <= {seq}"))
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    pub async fn len(&self) -> Result<usize> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(records.iter().map(|b| b.num_rows()).sum())
    }
}

fn entries_from_record_batch(batch: &RecordBatch) -> Result<Vec<OutboxEntry>> {
    let n = batch.num_rows();
    let seq_col = batch
        .column_by_name("seq")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let op_col = crate::util::optional_string_column(batch, "op").unwrap();
    let entity_col = crate::util::optional_string_column(batch, "entity").unwrap();
    let entity_id_col = crate::util::optional_string_column(batch, "entity_id").unwrap();
    let payload_col = crate::util::optional_string_column(batch, "payload").unwrap();
    let created_col = crate::util::optional_string_column(batch, "created_at").unwrap();

    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        entries.push(OutboxEntry {
            seq: seq_col.value(i),
            op: op_col.value(i).to_string(),
            entity: entity_col.value(i).to_string(),
            entity_id: entity_id_col.value(i).to_string(),
            payload: payload_col.value(i).to_string(),
            created_at: created_col.value(i).to_string(),
        });
    }
    Ok(entries)
}
