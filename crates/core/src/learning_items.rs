use unicode_normalization::UnicodeNormalization;

use serde::{Deserialize, Serialize};

use crate::curriculum::is_abstract_topic_name;
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
    /// RFC3339 timestamp of the last modification; `None` means "unknown"
    /// (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

impl LearningItem {
    pub fn slug_id(name: &str, target_lang: &str) -> String {
        let base = format!("{}-{}", target_lang, slugify(name));
        base.trim_matches('-').to_string()
    }

    pub fn from_topic(topic: &crate::curriculum::Topic) -> Self {
        Self {
            id: Self::slug_id(&topic.name, &topic.target_lang),
            name: topic.name.clone(),
            description: topic.description.clone(),
            level: topic.level.clone(),
            target_lang: topic.target_lang.clone(),
            native_lang: topic.native_lang.clone(),
            ..Default::default()
        }
    }
}

pub fn slugify(input: &str) -> String {
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
    let has_contrast_marker = lower.contains('/')
        || lower.contains(" vs ")
        || lower.contains(" and ")
        || lower.contains(" or ");

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
        if !stripped.is_ascii() {
            return true;
        }
        // Short ASCII tokens like "muy", "vs", "to" are usually part of the contrast.
        stripped.len() <= 4
    })
}

/// Grammar labels and generic connectors (English and Russian) that carry no
/// meaning when matching item names against session text or against each
/// other for deduplication.
const STOP_WORDS: &[&str] = &[
    "adverb",
    "verb",
    "verbs",
    "noun",
    "adjective",
    "article",
    "preposition",
    "vs",
    "and",
    "or",
    "the",
    "a",
    "an",
    "with",
    "for",
    "in",
    "on",
    "at",
    "of",
    "to",
    "с",
    "для",
    "и",
    "или",
    "по",
    "на",
    "в",
    "от",
    "из",
];

/// Content words of `text`: split on non-letter (Unicode) characters,
/// normalize each token (NFKD, alphanumeric-only, lowercase — same logic as
/// the report view's word matching), then drop stop words and tokens shorter
/// than 2 characters.
pub fn significant_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic())
        .map(normalize_token)
        .filter(|w| w.chars().count() >= 2 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Whether any of `words` appears in `text` as a whole normalized word —
/// never as a substring ("in" does not match "inside").
pub fn text_contains_any_word(text: &str, words: &[String]) -> bool {
    text.split(|c: char| !c.is_alphabetic())
        .map(normalize_token)
        .any(|w| !w.is_empty() && words.contains(&w))
}

fn normalize_token(word: &str) -> String {
    word.nfkd()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Fuzzy duplicate detection on name words. Returns the index of the first
/// `existing` name duplicating `candidate`: Jaccard similarity of the
/// significant-word sets ≥ 0.8, or one set containing the other (both
/// non-empty). Names with no significant words never count as duplicates.
pub fn is_duplicate_name(existing: &[String], candidate: &str) -> Option<usize> {
    let candidate_words: std::collections::HashSet<String> =
        significant_words(candidate).into_iter().collect();
    if candidate_words.is_empty() {
        return None;
    }
    for (i, name) in existing.iter().enumerate() {
        let existing_words: std::collections::HashSet<String> =
            significant_words(name).into_iter().collect();
        if existing_words.is_empty() {
            continue;
        }
        let intersection = candidate_words.intersection(&existing_words).count();
        if intersection == 0 {
            continue;
        }
        let union = candidate_words.union(&existing_words).count();
        let jaccard = intersection as f64 / union as f64;
        let subset = candidate_words.is_subset(&existing_words)
            || existing_words.is_subset(&candidate_words);
        if jaccard >= 0.8 || subset {
            return Some(i);
        }
    }
    None
}

/// Splits `items` into (kept, removed), merging fuzzy name duplicates (same
/// criterion as `is_duplicate_name`). From each duplicate group the item
/// with the highest practice_count is kept (ties: higher score, then
/// earliest in the input order), so the result is deterministic.
pub fn dedupe(items: Vec<LearningItem>) -> (Vec<LearningItem>, Vec<LearningItem>) {
    let mut kept: Vec<LearningItem> = Vec::new();
    let mut removed: Vec<LearningItem> = Vec::new();
    for item in items {
        let kept_names: Vec<String> = kept.iter().map(|k| k.name.clone()).collect();
        match is_duplicate_name(&kept_names, &item.name) {
            Some(pos) => {
                let replace = item.practice_count > kept[pos].practice_count
                    || (item.practice_count == kept[pos].practice_count
                        && item.score > kept[pos].score);
                if replace {
                    removed.push(std::mem::replace(&mut kept[pos], item));
                } else {
                    removed.push(item);
                }
            }
            None => kept.push(item),
        }
    }
    (kept, removed)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("pequeño/pequeña"), "pequeno-pequena");
        assert_eq!(
            slugify("Adverb: muy with adjectives"),
            "adverb-muy-with-adjectives"
        );
    }

    #[test]
    fn detects_learning_item_names() {
        assert!(is_learning_item_name("pequeño/pequeña"));
        assert!(is_learning_item_name(
            "Adjective agreement: pequeño/pequeña"
        ));
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
    fn significant_words_strip_labels_and_stop_words() {
        assert_eq!(
            significant_words("Adverb: вслух → out loud/aloud"),
            vec!["вслух", "out", "loud", "aloud"]
        );
        assert_eq!(
            significant_words("Verb: remind vs notice"),
            vec!["remind", "notice"]
        );
        assert!(significant_words("Verb vs Noun").is_empty());
        // Accents are normalized away.
        assert_eq!(
            significant_words("pequeño/pequeña"),
            vec!["pequeno", "pequena"]
        );
    }

    #[test]
    fn duplicate_name_detection() {
        let existing = vec!["Verb: remind vs notice".to_string()];
        assert!(is_duplicate_name(&existing, "Verbs: remind vs notice").is_some());

        let existing = vec!["in time vs on time".to_string()];
        assert!(is_duplicate_name(&existing, "On time vs In time").is_some());

        let existing = vec!["Prepositions with events".to_string()];
        assert!(is_duplicate_name(&existing, "Countable vs uncountable nouns").is_none());

        // Names that reduce to no significant words never dedupe.
        let existing = vec!["Verb vs Noun".to_string()];
        assert!(is_duplicate_name(&existing, "Adverb or Adjective").is_none());
    }

    #[test]
    fn dedupe_merges_duplicates_keeping_most_practiced() {
        let items = vec![
            make_item("a", "Verb: remind vs notice", 10.0, None, 1),
            make_item("b", "Verbs: remind vs notice", 40.0, None, 5),
            make_item("c", "Countable vs uncountable nouns", 20.0, None, 2),
        ];
        let (kept, removed) = dedupe(items);
        assert_eq!(kept.len(), 2);
        assert_eq!(removed.len(), 1);
        let kept_ids: Vec<&str> = kept.iter().map(|i| i.id.as_str()).collect();
        assert!(kept_ids.contains(&"b"));
        assert!(kept_ids.contains(&"c"));
        assert_eq!(removed[0].id, "a");
    }

    #[test]
    fn dedupe_keeps_distinct_items() {
        let items = vec![
            make_item("a", "pequeño/pequeña", 10.0, None, 1),
            make_item("b", "caro vs rico", 40.0, None, 5),
        ];
        let (kept, removed) = dedupe(items);
        assert_eq!(kept.len(), 2);
        assert!(removed.is_empty());
    }

    #[test]
    fn dedupe_tie_breaks_by_score_then_insertion_order() {
        let items = vec![
            make_item("first", "remind vs notice", 30.0, None, 2),
            // Same practice_count but higher score -> replaces the keeper.
            make_item("second", "Remind vs notice", 50.0, None, 2),
            // Fully tied with the keeper -> the earlier one stays.
            make_item("third", "remind VS notice", 50.0, None, 2),
        ];
        let (kept, removed) = dedupe(items);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "second");
        let removed_ids: Vec<&str> = removed.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(removed_ids, ["first", "third"]);
    }
}
