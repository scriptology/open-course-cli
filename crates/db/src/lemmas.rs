use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::{ExecutableQuery, QueryBase};

use crate::util::eq_predicate;
use open_course_core::curriculum::cefr_to_numeric;
use open_course_core::error::Result;

pub use open_course_core::vocabulary::*;

pub const TABLE_NAME: &str = "lemmas";

/// Mastery below which a lemma still counts as weak. Mirrors
/// `core::session::MASTERY_THRESHOLD`; the constant is deliberately
/// duplicated here to keep the db layer self-contained.
const LEMMA_MASTERY_THRESHOLD: f64 = 50.0;

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("lemma", DataType::Utf8, false),
        Field::new("pos", DataType::Utf8, false),
        Field::new("target_lang", DataType::Utf8, false),
        Field::new("native_lang", DataType::Utf8, false),
        Field::new("translation", DataType::Utf8, false),
        Field::new("status", DataType::Int32, false),
        Field::new("mastery", DataType::Float64, false),
        Field::new("last_seen", DataType::Utf8, true),
        Field::new("practice_count", DataType::Int32, false),
        Field::new("correct_uses", DataType::Int32, false),
        Field::new("incorrect_uses", DataType::Int32, false),
        Field::new("cefr_level", DataType::Utf8, true),
        Field::new("cefr_source", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, true),
        Field::new("deleted_at", DataType::Utf8, true),
    ]))
}

#[derive(Clone)]
pub struct LemmasTable {
    table: lancedb::Table,
}

impl LemmasTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .mode(CreateTableMode::exist_ok(|req| req))
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    /// Reads all non-deleted lemmas.
    pub async fn read_all(&self) -> Result<Vec<Lemma>> {
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
            all.extend(lemmas_from_record_batch(batch)?);
        }
        all.retain(|l| l.deleted_at.is_none());
        Ok(all)
    }

    /// Insert or replace a lemma, stamping `updated_at` with the current time.
    pub async fn upsert(&self, lemma: &Lemma) -> Result<()> {
        let mut lemma = lemma.clone();
        lemma.updated_at = Some(crate::util::now_rfc3339());
        self.upsert_with_timestamps(&lemma).await
    }

    /// Insert or replace a lemma exactly as given — used when applying
    /// synced changes whose timestamps must be preserved.
    pub async fn upsert_with_timestamps(&self, lemma: &Lemma) -> Result<()> {
        self.table
            .delete(&eq_predicate("id", &lemma.id))
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = lemma_to_record_batch(lemma)?;
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
            for mut lemma in lemmas_from_record_batch(batch)? {
                lemma.deleted_at = Some(now.clone());
                tombstoned.push(lemma);
            }
        }
        if tombstoned.is_empty() {
            return Ok(());
        }
        self.table
            .delete(&eq_predicate("id", id))
            .await
            .map_err(crate::error::DbError::from)?;
        for lemma in &tombstoned {
            let batch = lemma_to_record_batch(lemma)?;
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

    /// Return up to `n` weakest lemmas: lowest mastery first, then least
    /// recently seen (never-seen first). Only lemmas still being learned
    /// (status NEW or PRACTICING) with mastery below `LEMMA_MASTERY_THRESHOLD`
    /// qualify, so the result may be shorter than `n` (no padding).
    /// Function-word POS tags (ADP, DET, ...) are excluded via
    /// `is_content_pos` — forced practice targets content words only.
    ///
    /// `frontier_cefr` (numeric level of the lowest unfinished curriculum
    /// level, A1=1..C2=6) softly prioritizes level-appropriate vocabulary:
    /// lemmas with a CEFR level more than one step above the frontier sort
    /// after everything else but are never excluded. Lemmas without a level
    /// (or when `frontier_cefr` is `None`) keep the plain weakness order.
    pub fn weakest(lemmas: &[Lemma], n: usize, frontier_cefr: Option<i32>) -> Vec<Lemma> {
        // 0 = at/just above the frontier or unknown level; 1 = further ahead.
        let level_group = |l: &Lemma| match (frontier_cefr, l.cefr_level.as_deref()) {
            (Some(frontier), Some(level)) => match cefr_to_numeric(level) {
                Some(numeric) if numeric > frontier + 1 => 1,
                _ => 0,
            },
            _ => 0,
        };
        let mut qualified: Vec<Lemma> = lemmas
            .iter()
            .filter(|l| {
                (l.status == STATUS_NEW || l.status == STATUS_PRACTICING)
                    && l.mastery < LEMMA_MASTERY_THRESHOLD
                    && is_content_pos(&l.pos)
            })
            .cloned()
            .collect();
        qualified.sort_by(|a, b| {
            let group_cmp = level_group(a).cmp(&level_group(b));
            if group_cmp != std::cmp::Ordering::Equal {
                return group_cmp;
            }
            let mastery_cmp = a
                .mastery
                .partial_cmp(&b.mastery)
                .unwrap_or(std::cmp::Ordering::Equal);
            if mastery_cmp != std::cmp::Ordering::Equal {
                return mastery_cmp;
            }
            match (&a.last_seen, &b.last_seen) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(aa), Some(bb)) => aa.cmp(bb),
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        qualified.into_iter().take(n).collect()
    }
}

pub(crate) fn lemma_to_record_batch(lemma: &Lemma) -> Result<RecordBatch> {
    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from(vec![lemma.id.as_str()])),
            Arc::new(StringArray::from(vec![lemma.lemma.as_str()])),
            Arc::new(StringArray::from(vec![lemma.pos.as_str()])),
            Arc::new(StringArray::from(vec![lemma.target_lang.as_str()])),
            Arc::new(StringArray::from(vec![lemma.native_lang.as_str()])),
            Arc::new(StringArray::from(vec![lemma.translation.as_str()])),
            Arc::new(Int32Array::from(vec![lemma.status])),
            Arc::new(Float64Array::from(vec![lemma.mastery])),
            Arc::new(StringArray::from(vec![lemma.last_seen.as_deref()])),
            Arc::new(Int32Array::from(vec![lemma.practice_count])),
            Arc::new(Int32Array::from(vec![lemma.correct_uses])),
            Arc::new(Int32Array::from(vec![lemma.incorrect_uses])),
            Arc::new(StringArray::from(vec![lemma.cefr_level.as_deref()])),
            Arc::new(StringArray::from(vec![lemma.cefr_source.as_deref()])),
            Arc::new(StringArray::from(vec![lemma.updated_at.as_deref()])),
            Arc::new(StringArray::from(vec![lemma.deleted_at.as_deref()])),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn lemmas_from_record_batch(batch: &RecordBatch) -> Result<Vec<Lemma>> {
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
    let lemma_col = string_col("lemma");
    let pos_col = string_col("pos");
    let target_col = string_col("target_lang");
    let native_col = string_col("native_lang");
    let translation_col = string_col("translation");
    let status_col = int_col("status");
    let mastery_col = batch
        .column_by_name("mastery")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let last_seen_col = string_col("last_seen");
    let practice_count_col = int_col("practice_count");
    let correct_col = int_col("correct_uses");
    let incorrect_col = int_col("incorrect_uses");
    let updated_col = crate::util::optional_string_column(batch, "updated_at");
    let deleted_col = crate::util::optional_string_column(batch, "deleted_at");
    // Read tolerantly: dev databases created by intermediate builds may
    // lack the CEFR columns.
    let cefr_level_col = crate::util::optional_string_column(batch, "cefr_level");
    let cefr_source_col = crate::util::optional_string_column(batch, "cefr_source");

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(Lemma {
            id: id_col.value(i).to_string(),
            lemma: lemma_col.value(i).to_string(),
            pos: pos_col.value(i).to_string(),
            target_lang: target_col.value(i).to_string(),
            native_lang: native_col.value(i).to_string(),
            translation: translation_col.value(i).to_string(),
            status: status_col.value(i),
            mastery: mastery_col.value(i),
            last_seen: if last_seen_col.is_null(i) || last_seen_col.value(i).is_empty() {
                None
            } else {
                Some(last_seen_col.value(i).to_string())
            },
            practice_count: practice_count_col.value(i),
            correct_uses: correct_col.value(i),
            incorrect_uses: incorrect_col.value(i),
            cefr_level: crate::util::optional_string_at(cefr_level_col, i),
            cefr_source: crate::util::optional_string_at(cefr_source_col, i),
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

    fn make_lemma(id: &str, status: i32, mastery: f64, last_seen: Option<&str>) -> Lemma {
        Lemma {
            id: id.to_string(),
            lemma: id.to_string(),
            pos: "VERB".to_string(),
            target_lang: "es".to_string(),
            native_lang: "ru".to_string(),
            status,
            mastery,
            last_seen: last_seen.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn weakest_skips_known_and_graduated_lemmas() {
        let lemmas = vec![
            make_lemma("a", STATUS_NEW, 30.0, None),
            make_lemma("b", STATUS_PRACTICING, 50.0, None),
            make_lemma("c", STATUS_KNOWN, 20.0, None),
            make_lemma("d", STATUS_PRACTICING, 80.0, None),
        ];
        let weak = LemmasTable::weakest(&lemmas, 4, None);
        assert_eq!(weak.len(), 1);
        assert_eq!(weak[0].id, "a");
    }

    #[test]
    fn weakest_does_not_pad_to_n() {
        let lemmas = vec![
            make_lemma("a", STATUS_NEW, 10.0, None),
            make_lemma("b", STATUS_KNOWN, 90.0, None),
        ];
        let weak = LemmasTable::weakest(&lemmas, 5, None);
        assert_eq!(weak.len(), 1);
    }

    #[test]
    fn weakest_orders_by_mastery_then_recency() {
        let lemmas = vec![
            make_lemma("recent", STATUS_NEW, 40.0, Some("2024-01-10T00:00:00Z")),
            make_lemma("never", STATUS_NEW, 40.0, None),
            make_lemma(
                "older",
                STATUS_PRACTICING,
                40.0,
                Some("2024-01-01T00:00:00Z"),
            ),
            make_lemma(
                "weakest",
                STATUS_PRACTICING,
                10.0,
                Some("2024-02-01T00:00:00Z"),
            ),
        ];
        let weak = LemmasTable::weakest(&lemmas, 4, None);
        let ids: Vec<&str> = weak.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["weakest", "never", "older", "recent"]);
    }

    #[test]
    fn weakest_defers_lemmas_far_above_frontier() {
        let mut ahead = make_lemma("ahead", STATUS_PRACTICING, 10.0, None);
        ahead.cefr_level = Some("C1".to_string());
        let mut at_frontier = make_lemma("at-frontier", STATUS_NEW, 40.0, None);
        at_frontier.cefr_level = Some("A2".to_string());
        let mut one_above = make_lemma("one-above", STATUS_NEW, 40.0, None);
        one_above.cefr_level = Some("B1".to_string());
        let no_level = make_lemma("no-level", STATUS_NEW, 40.0, None);
        let lemmas = vec![ahead, at_frontier, one_above, no_level];

        // Frontier A2: the C1 lemma (more than one step up) sorts last but
        // is still returned.
        let weak = LemmasTable::weakest(&lemmas, 4, Some(2));
        let ids: Vec<&str> = weak.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["at-frontier", "one-above", "no-level", "ahead"]);

        // Deferred, not excluded: it is picked once the nearer lemmas no
        // longer fill the quota.
        let weak = LemmasTable::weakest(&lemmas, 3, Some(2));
        let ids: Vec<&str> = weak.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["at-frontier", "one-above", "no-level"]);
    }

    #[test]
    fn weakest_ignores_unparseable_cefr_levels() {
        let mut bogus = make_lemma("bogus", STATUS_PRACTICING, 10.0, None);
        bogus.cefr_level = Some("A0".to_string());
        let plain = make_lemma("plain", STATUS_NEW, 40.0, None);
        let lemmas = vec![bogus, plain];

        // An unparseable level is treated as unknown: plain weakness order.
        let weak = LemmasTable::weakest(&lemmas, 2, Some(1));
        let ids: Vec<&str> = weak.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["bogus", "plain"]);
    }

    #[test]
    fn weakest_skips_function_word_pos() {
        let mut adp = make_lemma("adp", STATUS_NEW, 10.0, None);
        adp.pos = "ADP".to_string();
        let mut det = make_lemma("det", STATUS_PRACTICING, 20.0, None);
        // POS matching is case-insensitive.
        det.pos = "det".to_string();
        let noun = make_lemma("noun", STATUS_PRACTICING, 30.0, None);
        // Empty and unrecognized POS tags stay candidates (lazy inclusion).
        let mut no_pos = make_lemma("no-pos", STATUS_NEW, 40.0, None);
        no_pos.pos = String::new();
        let lemmas = vec![adp, det, noun, no_pos];

        let weak = LemmasTable::weakest(&lemmas, 4, None);
        let ids: Vec<&str> = weak.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["noun", "no-pos"]);
    }

    #[tokio::test]
    async fn upsert_read_delete_roundtrip() {
        let dir = TempDir::new().unwrap();
        let connection = connect(&dir.path().join("db").to_string_lossy())
            .execute()
            .await
            .unwrap();
        let table = LemmasTable::open(&connection).await.unwrap();

        let lemma = Lemma {
            id: "es-comer".to_string(),
            lemma: "comer".to_string(),
            pos: "VERB".to_string(),
            target_lang: "es".to_string(),
            native_lang: "ru".to_string(),
            translation: "есть".to_string(),
            status: STATUS_PRACTICING,
            mastery: 55.0,
            last_seen: Some("2024-01-01T00:00:00Z".to_string()),
            practice_count: 3,
            correct_uses: 2,
            incorrect_uses: 1,
            cefr_level: Some("B1".to_string()),
            cefr_source: Some("topic".to_string()),
            ..Default::default()
        };
        table.upsert(&lemma).await.unwrap();

        let stored = table.read_all().await.unwrap();
        assert_eq!(stored.len(), 1);
        let mut expected = lemma.clone();
        expected.updated_at = stored[0].updated_at.clone();
        assert_eq!(stored[0], expected);
        // Upsert stamps updated_at.
        assert!(stored[0].updated_at.is_some());

        // Re-upsert replaces the row instead of duplicating it.
        table.upsert(&lemma).await.unwrap();
        assert_eq!(table.read_all().await.unwrap().len(), 1);

        // Soft-delete hides the row; purge removes the tombstone.
        table.delete_by_id("es-comer").await.unwrap();
        assert!(table.read_all().await.unwrap().is_empty());
        table.purge_deleted("2999-01-01T00:00:00Z").await.unwrap();
        table.upsert(&lemma).await.unwrap();
        assert_eq!(table.read_all().await.unwrap().len(), 1);

        table.reset().await.unwrap();
        assert!(table.read_all().await.unwrap().is_empty());
    }
}
