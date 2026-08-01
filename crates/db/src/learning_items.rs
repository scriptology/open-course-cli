use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::util::eq_predicate;
use open_course_core::error::Result;

pub use open_course_core::learning_items::*;

pub const TABLE_NAME: &str = "learning_items";

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, false),
        Field::new("level", DataType::Utf8, true),
        Field::new("target_lang", DataType::Utf8, false),
        Field::new("native_lang", DataType::Utf8, false),
        Field::new("score", DataType::Float64, false),
        Field::new("last_practiced", DataType::Utf8, true),
        Field::new("practice_count", DataType::Int32, false),
        Field::new("updated_at", DataType::Utf8, true),
        Field::new("deleted_at", DataType::Utf8, true),
    ]))
}

/// Score below which a learning item still counts as weak. Mirrors
/// `core::session::MASTERY_THRESHOLD`; the constant is deliberately
/// duplicated here to keep the db layer self-contained.
const ITEM_MASTERY_THRESHOLD: f64 = 50.0;

#[derive(Clone)]
pub struct LearningItemsTable {
    table: lancedb::Table,
}

impl LearningItemsTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    /// Reads all non-deleted learning items.
    pub async fn read_all(&self) -> Result<Vec<LearningItem>> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut all = Vec::new();
        for batch in &records {
            all.extend(learning_items_from_record_batch(batch)?);
        }
        all.retain(|i| i.deleted_at.is_none());
        Ok(all)
    }

    /// Insert or replace an item, stamping `updated_at` with the current time.
    pub async fn upsert(&self, item: &LearningItem) -> Result<()> {
        let mut item = item.clone();
        item.updated_at = Some(crate::util::now_rfc3339());
        self.upsert_with_timestamps(&item).await
    }

    /// Insert or replace an item exactly as given — used when applying
    /// synced changes whose timestamps must be preserved.
    pub async fn upsert_with_timestamps(&self, item: &LearningItem) -> Result<()> {
        self.table
            .delete(&eq_predicate("id", &item.id))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = learning_item_to_record_batch(item)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    /// Soft-delete: the row stays as a tombstone so sync can propagate the
    /// deletion; reads filter it out.
    pub async fn delete_by_id(&self, id: &str) -> Result<()> {
        let records = self
            .table
            .query()
            .only_if(eq_predicate("id", id))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let now = crate::util::now_rfc3339();
        let mut tombstoned = Vec::new();
        for batch in &records {
            for mut item in learning_items_from_record_batch(batch)? {
                item.deleted_at = Some(now.clone());
                tombstoned.push(item);
            }
        }
        if tombstoned.is_empty() {
            return Ok(());
        }
        self.table
            .delete(&eq_predicate("id", id))
            .await
            .map_err(crate::error::DbError::from)?;
        for item in &tombstoned {
            let batch = learning_item_to_record_batch(item)?;
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
            .delete("id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    /// Return up to `n` weakest learning items: lowest score first, then
    /// least recently practiced (never-practiced first). Only items with
    /// score below `ITEM_MASTERY_THRESHOLD` qualify — items at or above the
    /// threshold have graduated and are never returned, so the result may
    /// be shorter than `n` (no padding).
    pub fn weakest(items: &[LearningItem], n: usize) -> Vec<LearningItem> {
        let mut qualified: Vec<LearningItem> = items
            .iter()
            .filter(|i| i.score < ITEM_MASTERY_THRESHOLD)
            .cloned()
            .collect();
        qualified.sort_by(|a, b| {
            let score_cmp = a
                .score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal);
            if score_cmp != std::cmp::Ordering::Equal {
                return score_cmp;
            }
            match (&a.last_practiced, &b.last_practiced) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(aa), Some(bb)) => aa.cmp(bb),
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        qualified.into_iter().take(n).collect()
    }
}

pub(crate) fn learning_item_to_record_batch(item: &LearningItem) -> Result<RecordBatch> {
    let level = item.level.as_deref().unwrap_or("");
    let last = item.last_practiced.as_deref();
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![item.id.as_str()])),
            Arc::new(StringArray::from(vec![item.name.as_str()])),
            Arc::new(StringArray::from(vec![item.description.as_str()])),
            Arc::new(StringArray::from(vec![level])),
            Arc::new(StringArray::from(vec![item.target_lang.as_str()])),
            Arc::new(StringArray::from(vec![item.native_lang.as_str()])),
            Arc::new(Float64Array::from(vec![item.score])),
            Arc::new(StringArray::from(vec![last])),
            Arc::new(Int32Array::from(vec![item.practice_count])),
            Arc::new(StringArray::from(vec![item.updated_at.as_deref()])),
            Arc::new(StringArray::from(vec![item.deleted_at.as_deref()])),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn learning_items_from_record_batch(batch: &RecordBatch) -> Result<Vec<LearningItem>> {
    let n = batch.num_rows();
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
    let level_col = batch
        .column_by_name("level")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
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
    let count_col = batch
        .column_by_name("practice_count")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap();
    // Columns added in schema v2 may be absent in unmigrated tables.
    let updated_col = crate::util::optional_string_column(batch, "updated_at");
    let deleted_col = crate::util::optional_string_column(batch, "deleted_at");

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(LearningItem {
            id: id_col.value(i).to_string(),
            name: name_col.value(i).to_string(),
            description: desc_col.value(i).to_string(),
            level: if level_col.is_null(i) || level_col.value(i).is_empty() {
                None
            } else {
                Some(level_col.value(i).to_string())
            },
            target_lang: target_col.value(i).to_string(),
            native_lang: native_col.value(i).to_string(),
            score: score_col.value(i),
            last_practiced: if last_col.is_null(i) || last_col.value(i).is_empty() {
                None
            } else {
                Some(last_col.value(i).to_string())
            },
            practice_count: count_col.value(i),
            updated_at: crate::util::optional_string_at(updated_col, i),
            deleted_at: crate::util::optional_string_at(deleted_col, i),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(
        id: &str,
        name: &str,
        score: f64,
        last_practiced: Option<&str>,
        practice_count: i32,
    ) -> LearningItem {
        LearningItem {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            level: None,
            target_lang: "es".to_string(),
            native_lang: "ru".to_string(),
            score,
            last_practiced: last_practiced.map(|s| s.to_string()),
            practice_count,
            ..Default::default()
        }
    }

    #[test]
    fn weakest_skips_graduated_items() {
        let items = vec![
            make_item("a", "a-item", 30.0, None, 1),
            make_item("b", "b-item", 50.0, None, 1),
            make_item("c", "c-item", 80.0, None, 1),
        ];
        let weak = LearningItemsTable::weakest(&items, 3);
        assert_eq!(weak.len(), 1);
        assert_eq!(weak[0].id, "a");
    }

    #[test]
    fn weakest_does_not_pad_to_n() {
        let items = vec![
            make_item("a", "a-item", 10.0, None, 1),
            make_item("b", "b-item", 90.0, None, 1),
        ];
        let weak = LearningItemsTable::weakest(&items, 5);
        assert_eq!(weak.len(), 1);
    }

    #[test]
    fn weakest_orders_by_score_then_recency() {
        let items = vec![
            make_item(
                "recent",
                "recent-item",
                40.0,
                Some("2024-01-10T00:00:00Z"),
                1,
            ),
            make_item("never", "never-item", 40.0, None, 0),
            make_item("older", "older-item", 40.0, Some("2024-01-01T00:00:00Z"), 1),
            make_item(
                "weakest",
                "weakest-item",
                10.0,
                Some("2024-02-01T00:00:00Z"),
                5,
            ),
        ];
        let weak = LearningItemsTable::weakest(&items, 4);
        let ids: Vec<&str> = weak.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["weakest", "never", "older", "recent"]);
    }
}
