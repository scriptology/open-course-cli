use std::sync::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{Array, Int32Array, ListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::database::CreateTableMode;
use lancedb::query::ExecutableQuery;

use serde::{Deserialize, Serialize};

use crate::db::util::eq_predicate;
use crate::error::Result;

pub const TABLE_NAME: &str = "curriculum";
pub const DEFAULT_VERSION: i32 = 1;

pub const CEFR_LEVELS: &[&str] = &["A1", "A2", "B1", "B2", "C1", "C2"];

pub const CURRICULUM_DOMAIN_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "phonetics-orthography",
        "Stress, diacritics, alphabet rules, spelling conventions, letter-sound correspondences",
    ),
    (
        "morphology",
        "Nouns, articles, adjectives, pronouns, determiners, adverbs, prepositions and their agreement",
    ),
    (
        "syntax",
        "Word order, questions, negation, relative clauses, subordinate clauses, passive/impersonal constructions",
    ),
    (
        "verb-system",
        "Verb tenses, aspects, moods, regular and irregular verbs, stem-changing, reflexive, pronominal verbs",
    ),
    (
        "lexicon-vocabulary",
        "Thematic vocabulary sets, collocations, idioms, false friends, register-specific words",
    ),
    (
        "pragmatics-discourse",
        "Connectors, discourse markers, politeness, formal/informal register, speech acts",
    ),
    (
        "written-conventions",
        "Punctuation, capitalization, abbreviations, email/letter format, diacritics in writing",
    ),
    (
        "text-types",
        "Narrative, descriptive, argumentative, official, informal, and literary texts",
    ),
];

/// Target number of topics to generate for a single domain at a single CEFR level.
/// Keeping the count small makes each LLM request fast and avoids timeouts on slow
/// providers, while still covering the essential concepts.
pub fn target_topic_count(level: &str, _domain: &str) -> usize {
    match level.to_uppercase().as_str() {
        "A1" | "A2" => 4,
        "B1" | "B2" => 5,
        "C1" | "C2" => 6,
        _ => 4,
    }
}

/// Target number of topics to generate for a whole CEFR level in a single LLM call.
/// Keeping the count moderate makes each request fast enough to avoid timeouts while
/// still covering the essential grammar/vocabulary areas across all domains.
pub fn target_level_topic_count(level: &str) -> usize {
    match level.to_uppercase().as_str() {
        "A1" | "A2" => 12,
        "B1" | "B2" => 16,
        "C1" | "C2" => 20,
        _ => 12,
    }
}

const fn default_version() -> i32 {
    DEFAULT_VERSION
}

pub fn cefr_to_difficulty(level: &str) -> &'static str {
    match level.to_uppercase().as_str() {
        "A1" | "A2" => "beginner",
        "B1" | "B2" => "intermediate",
        "C1" | "C2" => "advanced",
        _ => "beginner",
    }
}

pub fn topic_domain(topic: &Topic) -> Option<&'static str> {
    for tag in &topic.tags {
        for (name, _) in CURRICULUM_DOMAIN_DESCRIPTIONS {
            if tag.eq_ignore_ascii_case(name) || tag.eq_ignore_ascii_case(&format!("domain:{name}"))
            {
                return Some(name);
            }
        }
    }
    None
}

/// Topic names that are too broad or abstract to be useful for repeated practice.
/// These are often invented by the analysis model as catch-all categories.
const ABSTRACT_TOPIC_PATTERNS: &[&str] = &[
    "common spelling errors",
    "common grammar mistakes",
    "common errors",
    "common mistakes",
    "grammar basics",
    "basic grammar",
    "basic vocabulary",
    "advanced vocabulary",
    "advanced grammar",
    "spelling errors",
    "grammar mistakes",
    "vocabulary",
    "fundamentals",
    "advanced topics",
];

pub fn is_abstract_topic_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ABSTRACT_TOPIC_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Returns true for topics that should be removed from the curriculum because
/// they are too abstract or are spelling-only catch-all topics.
pub fn should_remove_topic(name: &str) -> bool {
    is_abstract_topic_name(name) || name.to_lowercase().starts_with("spelling")
}

/// Removes abstract/spelling topics from curriculum, progress, and reviews tables,
/// and moves micro-topics (concrete learning items such as "X vs Y" or "Rule: example")
/// into the `learning_items` table while preserving their scores.
pub async fn cleanup_topics(db: &crate::db::Database) -> Result<(usize, usize)> {
    use crate::db::learning_items::{is_learning_item_name, LearningItem};

    let curriculum = db.curriculum().read_all().await?;
    let progress_data = db.progress().read_all().await?;
    let progress_by_id: std::collections::HashMap<String, crate::db::progress::ProgressTopic> =
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
            && (name.contains(':')
                || name.to_lowercase().contains(" vs ")
                || name.contains('/'));
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub id: String,
    pub name: String,
    pub description: String,
    pub difficulty: String,
    pub level: Option<String>,
    pub order: Option<i32>,
    pub tags: Vec<String>,
    pub target_lang: String,
    pub native_lang: String,
    #[serde(default = "default_version")]
    pub version: i32,
}

impl Topic {
    pub fn difficulty_enum(&self) -> Difficulty {
        match self.difficulty.as_str() {
            "intermediate" => Difficulty::Intermediate,
            "advanced" => Difficulty::Advanced,
            _ => Difficulty::Beginner,
        }
    }

    pub fn cefr_numeric(&self) -> i32 {
        cefr_to_numeric(self.level.as_deref().unwrap_or("")).unwrap_or(0)
    }

    pub fn sort_key(&self) -> i32 {
        self.order.unwrap_or_else(|| self.cefr_numeric())
    }
}

pub fn cefr_to_numeric(level: &str) -> Option<i32> {
    match level.to_uppercase().as_str() {
        "A1" => Some(1),
        "A2" => Some(2),
        "B1" => Some(3),
        "B2" => Some(4),
        "C1" => Some(5),
        "C2" => Some(6),
        _ => None,
    }
}

pub fn difficulty_to_cefr(difficulty: &str) -> Option<String> {
    match difficulty {
        "beginner" => Some("A1".to_string()),
        "intermediate" => Some("B1".to_string()),
        "advanced" => Some("C1".to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Beginner,
    Intermediate,
    Advanced,
}

impl Difficulty {
    pub fn as_str(&self) -> &'static str {
        match self {
            Difficulty::Beginner => "beginner",
            Difficulty::Intermediate => "intermediate",
            Difficulty::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Curriculum {
    #[serde(default = "default_version")]
    pub version: i32,
    pub topics: Vec<Topic>,
    pub target_language: String,
    pub native_language: String,
}

fn schema() -> Arc<Schema> {
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
            .await?;
        Ok(Self { table })
    }

    pub async fn read_all(&self) -> Result<Curriculum> {
        let records = self
            .table
            .query()
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
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

        sort_topics(&mut all_topics);

        Ok(Curriculum {
            version,
            topics: all_topics,
            target_language,
            native_language,
        })
    }

    pub async fn upsert(&self, topic: &Topic) -> Result<()> {
        self.table.delete(&eq_predicate("id", &topic.id)).await?;
        let batch = topic_to_record_batch(topic)?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<()> {
        self.table.delete(&eq_predicate("id", topic_id)).await?;
        Ok(())
    }

    pub async fn delete_all(&self) -> Result<()> {
        self.table.delete("id IS NOT NULL").await?;
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

fn topic_to_record_batch(topic: &Topic) -> Result<RecordBatch> {
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
        ],
    )?;
    Ok(batch)
}

fn topics_from_record_batch(batch: &RecordBatch) -> Result<Curriculum> {
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
        });
    }

    Ok(Curriculum {
        version: version_col.value(0),
        topics,
        target_language: target_col.value(0).to_string(),
        native_language: native_col.value(0).to_string(),
    })
}
