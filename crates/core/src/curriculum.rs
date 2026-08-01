use serde::{Deserialize, Serialize};

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

/// Deterministic topic id for newly generated topics: a slug of the topic
/// name (LLM-invented ids are not trusted), falling back to "topic" when the
/// name has no usable characters. Callers disambiguate collisions with `-1`,
/// `-2`, ... suffixes.
pub fn topic_id_from_name(name: &str) -> String {
    let slug = crate::learning_items::slugify(name);
    if slug.is_empty() {
        "topic".to_string()
    } else {
        slug
    }
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

/// Splits `topics` into (kept, removed), merging fuzzy name duplicates (same
/// criterion as `learning_items::is_duplicate_name`). The first topic of
/// each duplicate group is kept, so the result is deterministic.
pub fn dedupe(topics: Vec<Topic>) -> (Vec<Topic>, Vec<Topic>) {
    use crate::learning_items::is_duplicate_name;

    let mut kept: Vec<Topic> = Vec::new();
    let mut removed: Vec<Topic> = Vec::new();
    for topic in topics {
        let kept_names: Vec<String> = kept.iter().map(|t| t.name.clone()).collect();
        if is_duplicate_name(&kept_names, &topic.name).is_some() {
            removed.push(topic);
        } else {
            kept.push(topic);
        }
    }
    (kept, removed)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
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
    /// RFC3339 timestamp of the last local or synced modification; `None`
    /// means "unknown" (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads and kept
    /// only so sync can propagate the deletion.
    #[serde(default)]
    pub deleted_at: Option<String>,
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
#[cfg(test)]
mod tests {
    use super::*;

    fn make_topic(id: &str, name: &str) -> Topic {
        Topic {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            difficulty: "beginner".to_string(),
            level: None,
            order: None,
            tags: vec![],
            target_lang: "es".to_string(),
            native_lang: "ru".to_string(),
            version: 1,
            ..Default::default()
        }
    }

    #[test]
    fn dedupe_removes_duplicate_topic_names() {
        let topics = vec![
            make_topic("t1", "Conjugation patterns"),
            make_topic("t2", "Conjugation Patterns"),
            make_topic("t3", "Word stress rules"),
        ];
        let (kept, removed) = dedupe(topics);
        assert_eq!(kept.len(), 2);
        assert_eq!(removed.len(), 1);
        // The first topic of the duplicate group is kept.
        assert_eq!(kept[0].id, "t1");
        assert_eq!(kept[1].id, "t3");
        assert_eq!(removed[0].id, "t2");
    }

    #[test]
    fn dedupe_keeps_distinct_topics() {
        let topics = vec![
            make_topic("t1", "Prepositions with events"),
            make_topic("t2", "Countable vs uncountable nouns"),
        ];
        let (kept, removed) = dedupe(topics);
        assert_eq!(kept.len(), 2);
        assert!(removed.is_empty());
    }
}
