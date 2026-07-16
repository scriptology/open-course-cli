use std::sync::Arc;

use arrow_array::{
    Array, Float64Array, Int32Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::ExecutableQuery;

use crate::db::curriculum::is_abstract_topic_name;
use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "learning_items";

#[derive(Debug, Clone, PartialEq)]
pub struct LearningItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub level: Option<String>,
    pub target_lang: String,
    pub native_lang: String,
    pub score: f64,
    pub last_practiced: Option<String>,
    pub practice_count: i32,
}

impl LearningItem {
    pub fn slug_id(name: &str, target_lang: &str) -> String {
        let base = format!("{}-{}", target_lang, slugify(name));
        base.trim_matches('-').to_string()
    }

    pub fn from_topic(topic: &crate::db::curriculum::Topic) -> Self {
        Self {
            id: Self::slug_id(&topic.name, &topic.target_lang),
            name: topic.name.clone(),
            description: topic.description.clone(),
            level: topic.level.clone(),
            target_lang: topic.target_lang.clone(),
            native_lang: topic.native_lang.clone(),
            score: 0.0,
            last_practiced: None,
            practice_count: 0,
        }
    }
}

use unicode_normalization::UnicodeNormalization;

fn slugify(input: &str) -> String {
    // Decompose accented characters and drop combining marks.
    let decomposed: String = input.nfkd().collect();
    let without_marks: String = decomposed
        .chars()
        .filter(|c| !is_combining_mark(*c))
        .collect();
    let mut out = String::new();
    let mut prev_dash = true;
    for c in without_marks.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

fn is_combining_mark(c: char) -> bool {
    matches!(
        c as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

/// Heuristic that decides whether a generated topic is a small, concrete
/// learning target (word form, contrast pair, micro-pattern) rather than a
/// broad curriculum topic.
pub fn is_learning_item_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || is_abstract_topic_name(trimmed) {
        return false;
    }

    let lower = trimmed.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    // Explicit contrast markers strongly indicate a learning item.
    let has_contrast_marker =
        lower.contains('/') || lower.contains(" vs ") || lower.contains(" and ") || lower.contains(" or ");

    // A colon often introduces a specific example/contrast, e.g. "Adjective agreement: X/Y".
    let has_colon_example = lower.contains(':') && words.len() <= 7;

    // Short, concrete names (≤ 6 words) with a target-language word are likely learning items.
    let is_short_concrete = words.len() <= 6 && has_target_language_word(trimmed);

    has_contrast_marker || has_colon_example || is_short_concrete
}

fn has_target_language_word(name: &str) -> bool {
    // Treat any non-ASCII letters or short ASCII words as potential target-language words.
    // This is a cheap heuristic; we avoid matching broad English-only labels like
    // "Common Spelling Errors".
    name.split_whitespace().any(|w| {
        let stripped: String = w.chars().filter(|c| c.is_alphabetic()).collect();
        if stripped.is_empty() {
            return false;
        }
        // Non-ASCII letters strongly suggest a target-language word/form.
        if stripped.chars().any(|c| !c.is_ascii()) {
            return true;
        }
        // Short ASCII tokens like "muy", "vs", "to" are usually part of the contrast.
        stripped.len() <= 4
    })
}

fn schema() -> Arc<Schema> {
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
    ]))
}

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
            .await?;
        Ok(Self { table })
    }

    pub async fn read_all(&self) -> Result<Vec<LearningItem>> {
        let records = self
            .table
            .query()
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        let mut all = Vec::new();
        for batch in &records {
            all.extend(learning_items_from_record_batch(batch)?);
        }
        Ok(all)
    }

    pub async fn upsert(&self, item: &LearningItem) -> Result<()> {
        self.table.delete(&eq_predicate("id", &item.id)).await?;
        let batch = learning_item_to_record_batch(item)?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn delete_by_id(&self, id: &str) -> Result<()> {
        self.table.delete(&eq_predicate("id", id)).await?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table.delete("id IS NOT NULL").await?;
        Ok(())
    }

    /// Return the `n` weakest learning items: lowest score first, then least
    /// recently practiced. Prefers items with score < 50.
    pub fn weakest(items: &[LearningItem], n: usize) -> Vec<LearningItem> {
        let mut sorted: Vec<LearningItem> = items.to_vec();
        sorted.sort_by(|a, b| {
            let weak_a = if a.score < 50.0 { 0 } else { 1 };
            let weak_b = if b.score < 50.0 { 0 } else { 1 };
            match weak_a.cmp(&weak_b) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
            let score_cmp = a
                .score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal);
            if score_cmp != std::cmp::Ordering::Equal {
                return score_cmp;
            }
            match (&a.last_practiced,&b.last_practiced) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(aa), Some(bb)) => aa.cmp(bb),
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        sorted.into_iter().take(n).collect()
    }
}

fn learning_item_to_record_batch(item: &LearningItem) -> Result<RecordBatch> {
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
        ],
    )?;
    Ok(batch)
}

fn learning_items_from_record_batch(batch: &RecordBatch) -> Result<Vec<LearningItem>> {
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
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("pequeño/pequeña"), "pequeno-pequena");
        assert_eq!(slugify("Adverb: muy with adjectives"), "adverb-muy-with-adjectives");
    }

    #[test]
    fn detects_learning_item_names() {
        assert!(is_learning_item_name("pequeño/pequeña"));
        assert!(is_learning_item_name("Adjective agreement: pequeño/pequeña"));
        assert!(is_learning_item_name("Color adjectives: Rojo and Roja"));
        assert!(is_learning_item_name("Adverb muy with adjectives"));
        assert!(is_learning_item_name("Adjective: Caro vs Rico"));
        assert!(is_learning_item_name("marrón"));
    }

    #[test]
    fn rejects_broad_topics() {
        assert!(!is_learning_item_name("Adjective placement"));
        assert!(!is_learning_item_name("Common Spelling Errors"));
        assert!(!is_learning_item_name("Present tense verbs"));
    }
}
