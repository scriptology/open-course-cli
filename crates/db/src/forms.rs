use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::util::eq_predicate;
use open_course_core::error::Result;

pub use open_course_core::vocabulary::*;

pub const TABLE_NAME: &str = "forms";

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("lemma_id", DataType::Utf8, false),
        Field::new("surface", DataType::Utf8, false),
        Field::new("feats", DataType::Utf8, false),
        Field::new("feats_key", DataType::Utf8, false),
        Field::new("status", DataType::Int32, false),
        Field::new("mastery", DataType::Float64, false),
        Field::new("correct", DataType::Int32, false),
        Field::new("incorrect", DataType::Int32, false),
        Field::new("last_seen", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, true),
        Field::new("deleted_at", DataType::Utf8, true),
    ]))
}

#[derive(Clone)]
pub struct FormsTable {
    table: lancedb::Table,
}

impl FormsTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    /// Reads all non-deleted forms.
    pub async fn read_all(&self) -> Result<Vec<Form>> {
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
            all.extend(forms_from_record_batch(batch)?);
        }
        all.retain(|f| f.deleted_at.is_none());
        Ok(all)
    }

    /// Insert or replace a form, stamping `updated_at` with the current time.
    pub async fn upsert(&self, form: &Form) -> Result<()> {
        let mut form = form.clone();
        form.updated_at = Some(crate::util::now_rfc3339());
        self.upsert_with_timestamps(&form).await
    }

    /// Insert or replace a form exactly as given — used when applying
    /// synced changes whose timestamps must be preserved.
    pub async fn upsert_with_timestamps(&self, form: &Form) -> Result<()> {
        self.table
            .delete(&eq_predicate("id", &form.id))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = form_to_record_batch(form)?;
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
            for mut form in forms_from_record_batch(batch)? {
                form.deleted_at = Some(now.clone());
                tombstoned.push(form);
            }
        }
        if tombstoned.is_empty() {
            return Ok(());
        }
        self.table
            .delete(&eq_predicate("id", id))
            .await
            .map_err(crate::error::DbError::from)?;
        for form in &tombstoned {
            let batch = form_to_record_batch(form)?;
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
}

pub(crate) fn form_to_record_batch(form: &Form) -> Result<RecordBatch> {
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![form.id.as_str()])),
            Arc::new(StringArray::from(vec![form.lemma_id.as_str()])),
            Arc::new(StringArray::from(vec![form.surface.as_str()])),
            Arc::new(StringArray::from(vec![form.feats.as_str()])),
            Arc::new(StringArray::from(vec![form.feats_key.as_str()])),
            Arc::new(Int32Array::from(vec![form.status])),
            Arc::new(Float64Array::from(vec![form.mastery])),
            Arc::new(Int32Array::from(vec![form.correct])),
            Arc::new(Int32Array::from(vec![form.incorrect])),
            Arc::new(StringArray::from(vec![form.last_seen.as_deref()])),
            Arc::new(StringArray::from(vec![form.updated_at.as_deref()])),
            Arc::new(StringArray::from(vec![form.deleted_at.as_deref()])),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn forms_from_record_batch(batch: &RecordBatch) -> Result<Vec<Form>> {
    let n = batch.num_rows();
    let string_col = |name: &str| {
        batch
            .column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
    };
    let int_col = |name: &str| {
        batch
            .column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap()
    };
    let id_col = string_col("id");
    let lemma_id_col = string_col("lemma_id");
    let surface_col = string_col("surface");
    let feats_col = string_col("feats");
    let feats_key_col = string_col("feats_key");
    let status_col = int_col("status");
    let mastery_col = batch
        .column_by_name("mastery")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let correct_col = int_col("correct");
    let incorrect_col = int_col("incorrect");
    let last_seen_col = string_col("last_seen");
    let updated_col = crate::util::optional_string_column(batch, "updated_at");
    let deleted_col = crate::util::optional_string_column(batch, "deleted_at");

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(Form {
            id: id_col.value(i).to_string(),
            lemma_id: lemma_id_col.value(i).to_string(),
            surface: surface_col.value(i).to_string(),
            feats: feats_col.value(i).to_string(),
            feats_key: feats_key_col.value(i).to_string(),
            status: status_col.value(i),
            mastery: mastery_col.value(i),
            correct: correct_col.value(i),
            incorrect: incorrect_col.value(i),
            last_seen: if last_seen_col.is_null(i) || last_seen_col.value(i).is_empty() {
                None
            } else {
                Some(last_seen_col.value(i).to_string())
            },
            updated_at: crate::util::optional_string_at(updated_col, i),
            deleted_at: crate::util::optional_string_at(deleted_col, i),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lancedb::connect;
    use tempfile::TempDir;

    #[tokio::test]
    async fn upsert_read_delete_roundtrip() {
        let dir = TempDir::new().unwrap();
        let connection = connect(&dir.path().join("db").to_string_lossy())
            .execute()
            .await
            .unwrap();
        let table = FormsTable::open(&connection).await.unwrap();

        let form = Form {
            id: "es-comer--comi".to_string(),
            lemma_id: "es-comer".to_string(),
            surface: "comí".to_string(),
            feats: "Mood=Ind|Tense=Past".to_string(),
            // Stored keys are normalized (lowercase, canonicalized).
            feats_key: "mood=ind|tense=past".to_string(),
            status: STATUS_PRACTICING,
            mastery: 60.0,
            correct: 2,
            incorrect: 1,
            last_seen: Some("2024-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };
        table.upsert(&form).await.unwrap();

        let stored = table.read_all().await.unwrap();
        assert_eq!(stored.len(), 1);
        let mut expected = form.clone();
        expected.updated_at = stored[0].updated_at.clone();
        assert_eq!(stored[0], expected);
        // Upsert stamps updated_at.
        assert!(stored[0].updated_at.is_some());

        // Re-upsert replaces the row instead of duplicating it.
        table.upsert(&form).await.unwrap();
        assert_eq!(table.read_all().await.unwrap().len(), 1);

        // Soft-delete hides the row; purge removes the tombstone.
        table.delete_by_id("es-comer--comi").await.unwrap();
        assert!(table.read_all().await.unwrap().is_empty());
        table.purge_deleted("2999-01-01T00:00:00Z").await.unwrap();
        table.upsert(&form).await.unwrap();
        assert_eq!(table.read_all().await.unwrap().len(), 1);

        table.reset().await.unwrap();
        assert!(table.read_all().await.unwrap().is_empty());
    }
}
