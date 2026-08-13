//! Vocabulary domain model: lemmas (dictionary headwords) and their
//! inflected forms, described with Universal Dependencies morphology.
//! Flat fields, snake_case JSON, and the same `updated_at`/`deleted_at`
//! LWW-sync contract as `learning_items.rs`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::learning_items::{significant_words, slugify, text_contains_any_word};
use crate::session::{COMPLETED_THRESHOLD, GrammarError, MASTERY_THRESHOLD, SentenceAnalysis};

/// Content-word POS tags eligible for forced vocabulary practice.
pub const CONTENT_POS: &[&str] = &["NOUN", "VERB", "ADJ", "ADV", "PROPN"];

/// Function-word POS tags (the closed UD set) excluded from forced
/// vocabulary practice.
const FUNCTION_POS: &[&str] = &[
    "ADP", "AUX", "CCONJ", "DET", "INTJ", "PART", "PRON", "PUNCT", "SCONJ", "SYM", "X",
];

/// Whether a lemma with this POS tag is a content-word candidate for
/// forced practice. Case-insensitive; empty or unrecognized tags are
/// lazily accepted so incomplete metadata never hides a lemma.
pub fn is_content_pos(pos: &str) -> bool {
    let trimmed = pos.trim();
    if trimmed.is_empty() {
        return true;
    }
    !FUNCTION_POS.iter().any(|p| p.eq_ignore_ascii_case(trimmed))
}

/// Freshly extracted, never practiced.
pub const STATUS_NEW: i32 = 0;
/// Practiced but not yet mastered (mastery ≥ 50).
pub const STATUS_PRACTICING: i32 = 1;
/// Mastered (mastery ≥ 80).
pub const STATUS_KNOWN: i32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Lemma {
    /// Deterministic id: `"{target_lang}-{slug(lemma)}"`; callers
    /// disambiguate (lemma, pos) collisions with a `-{pos}` suffix.
    pub id: String,
    pub lemma: String,
    /// Universal Dependencies part-of-speech tag ("VERB", "ADJ", ...).
    pub pos: String,
    pub target_lang: String,
    pub native_lang: String,
    /// Main translation; may be empty until the pipeline fills it.
    pub translation: String,
    /// Derived from mastery via `derive_status`; stored for reads/UI.
    pub status: i32,
    pub mastery: f64,
    /// RFC3339 timestamp of the last session that touched this lemma.
    pub last_seen: Option<String>,
    pub practice_count: i32,
    pub correct_uses: i32,
    pub incorrect_uses: i32,
    /// Approximate CEFR level of the lemma ("A1"–"C2"); `None` until a
    /// source assigns one.
    #[serde(default)]
    pub cefr_level: Option<String>,
    /// Where `cefr_level` came from: "topic" (curriculum level of the topic
    /// the lemma appeared in), "llm" (model estimate), "manual" (user-set),
    /// or "list" (imported word list). A new value replaces the stored one
    /// only when its source ranks strictly higher (see `cefr_source_rank`).
    #[serde(default)]
    pub cefr_source: Option<String>,
    /// RFC3339 timestamp of the last modification; `None` means "unknown"
    /// (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

impl Lemma {
    pub fn slug_id(lemma: &str, target_lang: &str) -> String {
        let base = format!("{}-{}", target_lang, slugify(lemma));
        base.trim_matches('-').to_string()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Form {
    /// Deterministic id: `"{lemma_id}--{slug(surface)}"`; callers
    /// disambiguate feats_key collisions with `-1`, `-2`, ... suffixes.
    pub id: String,
    pub lemma_id: String,
    pub surface: String,
    /// UD features as returned by the LLM ("Mood=Ind|Number=Sing|...").
    pub feats: String,
    /// Normalized (sorted) form of `feats`, used for deduplication.
    pub feats_key: String,
    pub status: i32,
    pub mastery: f64,
    pub correct: i32,
    pub incorrect: i32,
    /// RFC3339 timestamp of the last session that touched this form.
    pub last_seen: Option<String>,
    /// RFC3339 timestamp of the last modification; `None` means "unknown"
    /// (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

impl Form {
    pub fn id(lemma_id: &str, surface: &str) -> String {
        format!("{}--{}", lemma_id, slugify(surface))
    }
}

/// Canonical short form of a full lowercased `key=value` UD pair, or the
/// input unchanged when no canonicalization applies. Only complete pairs
/// are rewritten — never bare keys or values — to avoid collisions like
/// "Imperfect" vs "Imperative".
fn canonicalize_feat(segment: &str) -> &str {
    match segment {
        "tense=present" => "tense=pres",
        "tense=preterite" => "tense=past",
        "mood=indicative" => "mood=ind",
        "number=singular" => "number=sing",
        "number=plural" => "number=plur",
        "gender=masculine" => "gender=masc",
        "gender=feminine" => "gender=fem",
        "person=first" => "person=1",
        "person=second" => "person=2",
        "person=third" => "person=3",
        "verbform=finite" => "verbform=fin",
        other => other,
    }
}

/// Normalized dedup key for UD features: split on '|', trim, drop empty
/// segments, lowercase, canonicalize known `key=value` pairs, sort,
/// dedup, rejoin. Best-effort — malformed segments are kept as-is
/// (garbage feats must not block form creation).
pub fn normalize_feats_key(feats: &str) -> String {
    let mut parts: Vec<String> = feats
        .split('|')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let lowered = p.to_lowercase();
            canonicalize_feat(&lowered).to_string()
        })
        .collect();
    parts.sort_unstable();
    parts.dedup();
    parts.join("|")
}

/// Unicode-aware comparison key for lemmas and surface forms: NFKC
/// normalization plus lowercase. Diacritics are kept ("año" ≠ "ano").
pub fn normalize_key(s: &str) -> String {
    s.nfkc().collect::<String>().to_lowercase()
}

/// Maximum number of warm-up cards shown before a session's exercises.
pub const MAX_WARMUP_ITEMS: usize = 8;

/// `Some(value)` trimmed when non-empty, `None` otherwise. The LLM sometimes
/// emits empty strings instead of omitting an optional field.
fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Builds warm-up cards for a session's forced lemmas from the LLM's raw
/// `warmup` output. Cards follow the forced-lemma order and are capped at
/// `MAX_WARMUP_ITEMS`. A raw entry whose lemma matches (via `normalize_key`)
/// supplies the translation and example; unmatched forced lemmas fall back
/// to the stored translation with no example. Lemmas without any translation
/// are skipped (there is nothing to teach), and raw entries matching no
/// forced lemma are dropped.
pub fn match_warmup_items(
    forced: &[Lemma],
    raw: Vec<crate::llm::parse::RawWarmupItem>,
) -> Vec<crate::session::WarmupItem> {
    use std::collections::HashMap;

    let mut by_key: HashMap<String, crate::llm::parse::RawWarmupItem> = HashMap::new();
    for item in raw {
        let key = normalize_key(item.lemma.trim());
        if key.is_empty() {
            continue;
        }
        by_key.entry(key).or_insert(item);
    }

    forced
        .iter()
        .take(MAX_WARMUP_ITEMS)
        .filter_map(|lemma| {
            let key = normalize_key(&lemma.lemma);
            let item = match by_key.get(&key) {
                Some(raw) => {
                    let translation = non_empty(raw.translation.clone())
                        .unwrap_or_else(|| lemma.translation.clone());
                    crate::session::WarmupItem {
                        lemma_id: Some(lemma.id.clone()),
                        lemma: lemma.lemma.clone(),
                        pos: non_empty(raw.pos.clone())
                            .or_else(|| non_empty(Some(lemma.pos.clone()))),
                        cefr_level: non_empty(raw.cefr_level.clone())
                            .or_else(|| lemma.cefr_level.clone()),
                        translation,
                        example: non_empty(raw.example.clone()),
                    }
                }
                None => crate::session::WarmupItem {
                    lemma_id: Some(lemma.id.clone()),
                    lemma: lemma.lemma.clone(),
                    pos: non_empty(Some(lemma.pos.clone())),
                    cefr_level: lemma.cefr_level.clone(),
                    translation: lemma.translation.clone(),
                    example: None,
                },
            };
            if item.translation.is_empty() {
                None
            } else {
                Some(item)
            }
        })
        .collect()
}

/// Priority rank of a CEFR source: user-curated data ("manual"/"list") is 4,
/// curriculum topics ("topic") 3, LLM estimates ("llm") 2, and anything else
/// or `None` is 0.
pub fn cefr_source_rank(source: Option<&str>) -> u8 {
    match source {
        Some("manual") | Some("list") => 4,
        Some("topic") => 3,
        Some("llm") => 2,
        _ => 0,
    }
}

/// Whether a CEFR value from `new_source` should replace the stored one from
/// `existing_source`: only when the new source ranks strictly higher, so an
/// LLM estimate never overwrites a topic- or user-derived level.
pub fn should_replace_cefr(existing_source: Option<&str>, new_source: &str) -> bool {
    cefr_source_rank(Some(new_source)) > cefr_source_rank(existing_source)
}

/// Vocabulary status derived from mastery: 0 below 50, 1 from 50, 2 from 80.
/// An observed error caps the status at `STATUS_PRACTICING`: one mistake
/// means the word is not fully known yet.
pub fn derive_status(mastery: f64, had_error: bool) -> i32 {
    let base = if mastery >= COMPLETED_THRESHOLD {
        STATUS_KNOWN
    } else if mastery >= MASTERY_THRESHOLD {
        STATUS_PRACTICING
    } else {
        STATUS_NEW
    };
    if had_error {
        base.min(STATUS_PRACTICING)
    } else {
        base
    }
}

/// Per-use session score for a student-side vocabulary use: 100 when both
/// spelling and usage are fine, 30 for a spelling-only problem, 0 when the
/// word is misused (wrong choice, form, or agreement).
pub fn vocabulary_session_score(spelling_ok: bool, usage_ok: bool) -> f64 {
    if !usage_ok {
        0.0
    } else if !spelling_ok {
        30.0
    } else {
        100.0
    }
}

/// Index of the existing lemma matching `(target_lang, lemma, pos)`. The
/// headword and POS are compared with `normalize_key` because the LLM does
/// not guarantee consistent casing or Unicode normalization.
pub fn find_lemma(existing: &[Lemma], target_lang: &str, lemma: &str, pos: &str) -> Option<usize> {
    existing.iter().position(|l| {
        l.target_lang == target_lang
            && normalize_key(&l.lemma) == normalize_key(lemma)
            && normalize_key(&l.pos) == normalize_key(pos)
    })
}

/// Index of the existing form matching `(lemma_id, feats_key)`. Falls back
/// to a surface match when `feats_key` is empty, so malformed LLM feats do
/// not produce duplicate forms.
pub fn find_form(
    existing: &[Form],
    lemma_id: &str,
    surface: &str,
    feats_key: &str,
) -> Option<usize> {
    existing.iter().position(|f| {
        f.lemma_id == lemma_id
            && if feats_key.is_empty() {
                normalize_key(&f.surface) == normalize_key(surface)
            } else {
                f.feats_key == feats_key
            }
    })
}

/// Minimum feats similarity for `find_form_fuzzy` to treat two forms of
/// the same surface as one inflected form.
pub const FORM_MERGE_SIMILARITY: f64 = 0.8;

/// Similarity of two normalized feats keys: Jaccard over their segment
/// sets, with a subset (or two empty keys) counting as 1.0.
pub fn feats_similarity(a: &str, b: &str) -> f64 {
    let set_a: HashSet<&str> = a.split('|').filter(|s| !s.is_empty()).collect();
    let set_b: HashSet<&str> = b.split('|').filter(|s| !s.is_empty()).collect();
    if set_a.is_subset(&set_b) || set_b.is_subset(&set_a) {
        return 1.0;
    }
    let intersection = set_a.intersection(&set_b).count() as f64;
    let union = set_a.union(&set_b).count() as f64;
    intersection / union
}

/// Index of the existing form matching `(lemma_id, surface, feats_key)`:
/// first the exact `find_form`, otherwise the same-lemma form with the
/// same normalized surface whose feats similarity is highest and at least
/// `FORM_MERGE_SIMILARITY` (e.g. an incomplete feats set from the LLM).
pub fn find_form_fuzzy(
    existing: &[Form],
    lemma_id: &str,
    surface: &str,
    feats_key: &str,
) -> Option<usize> {
    if let Some(i) = find_form(existing, lemma_id, surface, feats_key) {
        return Some(i);
    }
    let surface_key = normalize_key(surface);
    existing
        .iter()
        .enumerate()
        .filter(|(_, f)| f.lemma_id == lemma_id && normalize_key(&f.surface) == surface_key)
        .map(|(i, f)| (i, feats_similarity(&f.feats_key, feats_key)))
        .filter(|(_, sim)| *sim >= FORM_MERGE_SIMILARITY)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// Whether a grammar error is fully explained by a single word form the
/// student got wrong: true when a student-side vocabulary use failed
/// (`spelling_ok` or `usage_ok` is `Some(false)`) and its surface or lemma
/// shares significant words with the error's pattern/explanation. Such
/// errors belong to the vocabulary system and must not also create
/// learning items.
pub fn vocabulary_owns_error(sentence: &SentenceAnalysis, error: &GrammarError) -> bool {
    let error_text = format!("{} {}", error.pattern, error.explanation);
    let error_words = significant_words(&error_text);
    if error_words.is_empty() {
        return false;
    }
    sentence
        .used_vocabulary
        .iter()
        .filter(|u| u.side == "student")
        .filter(|u| u.spelling_ok == Some(false) || u.usage_ok == Some(false))
        .any(|u| {
            text_contains_any_word(&u.surface, &error_words)
                || text_contains_any_word(&u.lemma, &error_words)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_feats_key_sorts_and_trims() {
        assert_eq!(
            normalize_feats_key("Tense=Pres | Mood=Ind|Number=Sing"),
            "mood=ind|number=sing|tense=pres"
        );
    }

    #[test]
    fn normalize_feats_key_tolerates_garbage() {
        assert_eq!(normalize_feats_key(""), "");
        assert_eq!(normalize_feats_key("|| |"), "");
        // Malformed segments are kept, not dropped.
        assert_eq!(
            normalize_feats_key("bogus|Number=Sing"),
            "bogus|number=sing"
        );
    }

    #[test]
    fn normalize_feats_key_canonicalizes_full_pairs() {
        assert_eq!(normalize_feats_key("Tense=Present"), "tense=pres");
        assert_eq!(normalize_feats_key("Tense=Preterite"), "tense=past");
        assert_eq!(
            normalize_feats_key(
                "Mood=Indicative|Number=Singular|Person=First|Gender=Feminine|VerbForm=Finite"
            ),
            "gender=fem|mood=ind|number=sing|person=1|verbform=fin"
        );
        // Unknown pairs are only lowercased, never guessed.
        assert_eq!(normalize_feats_key("Tense=Imperfect"), "tense=imperfect");
    }

    #[test]
    fn normalize_feats_key_dedups_after_canonicalization() {
        assert_eq!(
            normalize_feats_key("Number=Sing|Number=Singular|number=sing"),
            "number=sing"
        );
    }

    #[test]
    fn normalize_feats_key_is_order_and_case_insensitive() {
        // "hablo": same feats in a different order and casing produce one key.
        assert_eq!(
            normalize_feats_key("Mood=Ind|Number=Sing|Person=1|Tense=Pres"),
            normalize_feats_key("Tense=Pres | person=1|Number=Sing|MOOD=IND")
        );
        // Full UD values canonicalize to the same key as short ones.
        assert_eq!(
            normalize_feats_key("Mood=Indicative|Tense=Present|Number=Singular|Person=1"),
            normalize_feats_key("mood=ind|tense=pres|number=sing|person=1")
        );
    }

    #[test]
    fn normalize_key_keeps_diacritics() {
        assert_eq!(normalize_key("Año"), "año");
        assert_ne!(normalize_key("año"), normalize_key("ano"));
        // NFKC folds compatibility characters.
        assert_eq!(normalize_key("ﬁnal"), "final");
    }

    #[test]
    fn is_content_pos_filters_function_words() {
        for pos in ["NOUN", "verb", "Adj", "ADV", "PROPN"] {
            assert!(is_content_pos(pos), "{pos} should be content");
        }
        for pos in ["PART", "ADP", "DET", "PRON", "AUX", "CCONJ", "PUNCT"] {
            assert!(!is_content_pos(pos), "{pos} should be function");
        }
        // Empty or unrecognized tags are lazily accepted.
        assert!(is_content_pos(""));
        assert!(is_content_pos("NUM"));
        assert!(is_content_pos("garbage"));
    }

    #[test]
    fn derive_status_thresholds() {
        assert_eq!(derive_status(0.0, false), STATUS_NEW);
        assert_eq!(derive_status(49.9, false), STATUS_NEW);
        assert_eq!(derive_status(50.0, false), STATUS_PRACTICING);
        assert_eq!(derive_status(79.9, false), STATUS_PRACTICING);
        assert_eq!(derive_status(80.0, false), STATUS_KNOWN);
        assert_eq!(derive_status(100.0, false), STATUS_KNOWN);
    }

    #[test]
    fn derive_status_error_caps_at_practicing() {
        assert_eq!(derive_status(95.0, true), STATUS_PRACTICING);
        assert_eq!(derive_status(60.0, true), STATUS_PRACTICING);
        assert_eq!(derive_status(10.0, true), STATUS_NEW);
    }

    #[test]
    fn vocabulary_session_score_values() {
        assert_eq!(vocabulary_session_score(true, true), 100.0);
        assert_eq!(vocabulary_session_score(false, true), 30.0);
        assert_eq!(vocabulary_session_score(true, false), 0.0);
        assert_eq!(vocabulary_session_score(false, false), 0.0);
    }

    #[test]
    fn ids_are_deterministic() {
        assert_eq!(Lemma::slug_id("comer", "es"), "es-comer");
        assert_eq!(
            Lemma::slug_id("Pequeño/Pequeña", "es"),
            "es-pequeno-pequena"
        );
        assert_eq!(Form::id("es-comer", "comí"), "es-comer--comi");
        assert_eq!(Form::id("es-comer", "Comí"), "es-comer--comi");
    }

    #[test]
    fn cefr_source_rank_orders_sources() {
        assert_eq!(cefr_source_rank(Some("manual")), 4);
        assert_eq!(cefr_source_rank(Some("list")), 4);
        assert_eq!(cefr_source_rank(Some("topic")), 3);
        assert_eq!(cefr_source_rank(Some("llm")), 2);
        assert_eq!(cefr_source_rank(Some("bogus")), 0);
        assert_eq!(cefr_source_rank(None), 0);
    }

    #[test]
    fn should_replace_cefr_requires_strictly_higher_rank() {
        // llm -> topic replaces.
        assert!(should_replace_cefr(Some("llm"), "topic"));
        // topic -> llm does not.
        assert!(!should_replace_cefr(Some("topic"), "llm"));
        // Missing source is replaced by any known source.
        assert!(should_replace_cefr(None, "llm"));
        // Unknown new source never replaces.
        assert!(!should_replace_cefr(None, "bogus"));
        // Equal rank does not replace.
        assert!(!should_replace_cefr(Some("topic"), "topic"));
        assert!(!should_replace_cefr(Some("manual"), "list"));
    }

    #[test]
    fn find_lemma_matches_case_insensitively() {
        let existing = vec![Lemma {
            id: "es-comer".to_string(),
            lemma: "comer".to_string(),
            pos: "VERB".to_string(),
            target_lang: "es".to_string(),
            ..Default::default()
        }];
        assert_eq!(find_lemma(&existing, "es", "Comer", "verb"), Some(0));
        assert_eq!(find_lemma(&existing, "es", "comer", "NOUN"), None);
        assert_eq!(find_lemma(&existing, "fr", "comer", "VERB"), None);
    }

    #[test]
    fn find_form_dedups_by_feats_key_then_surface() {
        let existing = vec![
            Form {
                id: "es-comer--como".to_string(),
                lemma_id: "es-comer".to_string(),
                surface: "como".to_string(),
                feats_key: "Mood=Ind|Number=Sing|Person=1".to_string(),
                ..Default::default()
            },
            Form {
                id: "es-comer--comi".to_string(),
                lemma_id: "es-comer".to_string(),
                surface: "comí".to_string(),
                feats_key: String::new(),
                ..Default::default()
            },
        ];
        // Same lemma and feats_key -> duplicate.
        assert_eq!(
            find_form(
                &existing,
                "es-comer",
                "como",
                "Mood=Ind|Number=Sing|Person=1"
            ),
            Some(0)
        );
        // Different feats_key -> distinct form.
        assert_eq!(
            find_form(
                &existing,
                "es-comer",
                "como",
                "Mood=Ind|Number=Plur|Person=1"
            ),
            None
        );
        // Empty feats_key falls back to the surface match.
        assert_eq!(find_form(&existing, "es-comer", "Comí", ""), Some(1));
        assert_eq!(find_form(&existing, "es-comer", "como", ""), Some(0));
    }

    #[test]
    fn find_lemma_does_not_merge_diacritics() {
        let existing = vec![Lemma {
            id: "es-ano".to_string(),
            lemma: "ano".to_string(),
            pos: "NOUN".to_string(),
            target_lang: "es".to_string(),
            ..Default::default()
        }];
        assert_eq!(find_lemma(&existing, "es", "año", "NOUN"), None);
        assert_eq!(find_lemma(&existing, "es", "Ano", "noun"), Some(0));
    }

    #[test]
    fn feats_similarity_jaccard_with_subset_shortcut() {
        assert_eq!(feats_similarity("", ""), 1.0);
        // A subset (e.g. an incomplete feats set: 4 of 5 segments) is a
        // full match.
        assert_eq!(
            feats_similarity(
                "mood=ind|number=sing|person=1|tense=pres",
                "mood=ind|number=sing|person=1|tense=pres|verbform=fin"
            ),
            1.0
        );
        // 8 of 10 union segments -> 0.8, right at the merge threshold.
        let a = "a=1|b=1|c=1|d=1|e=1|f=1|g=1|h=1|i=1";
        let b = "a=1|b=1|c=1|d=1|e=1|f=1|g=1|h=1|j=1";
        assert_eq!(feats_similarity(a, b), 0.8);
        // 4 of 6 union segments -> 0.667, below the threshold.
        let a = "mood=ind|number=sing|person=1|tense=pres|verbform=fin";
        let b = "mood=ind|number=sing|person=1|tense=past|verbform=fin";
        assert!((feats_similarity(a, b) - 2.0 / 3.0).abs() < 1e-9);
        // Disjoint sets -> 0.0.
        assert_eq!(feats_similarity("mood=ind", "mood=sub"), 0.0);
    }

    #[test]
    fn find_form_fuzzy_merges_incomplete_feats() {
        let existing = vec![
            Form {
                id: "es-hablar--hablo".to_string(),
                lemma_id: "es-hablar".to_string(),
                surface: "hablo".to_string(),
                feats_key: "mood=ind|number=sing|person=1|tense=pres|verbform=fin".to_string(),
                ..Default::default()
            },
            Form {
                id: "es-hablar--hablo-1".to_string(),
                lemma_id: "es-hablar".to_string(),
                surface: "hablo".to_string(),
                feats_key: "mood=sub|number=sing|person=1|tense=pres|verbform=fin".to_string(),
                ..Default::default()
            },
        ];
        // Exact feats_key match wins.
        assert_eq!(
            find_form_fuzzy(
                &existing,
                "es-hablar",
                "Hablo",
                "mood=ind|number=sing|person=1|tense=pres|verbform=fin"
            ),
            Some(0)
        );
        // Incomplete feats (4 of 5 segments, a subset) merge into the
        // fuller form.
        assert_eq!(
            find_form_fuzzy(
                &existing,
                "es-hablar",
                "hablo",
                "mood=ind|number=sing|person=1|tense=pres"
            ),
            Some(0)
        );
        // Different feature sets (past vs present, similarity 0.67) stay
        // distinct forms.
        assert_eq!(
            find_form_fuzzy(
                &existing,
                "es-hablar",
                "hablo",
                "mood=ind|number=sing|person=1|tense=past|verbform=fin"
            ),
            None
        );
        // A different lemma never merges.
        assert_eq!(
            find_form_fuzzy(
                &existing,
                "es-comer",
                "hablo",
                "mood=ind|number=sing|person=1|tense=pres|verbform=fin"
            ),
            None
        );
    }

    fn sentence_with_uses(uses: Vec<crate::session::VocabularyUse>) -> SentenceAnalysis {
        SentenceAnalysis {
            sentence_number: 1,
            student_translation: String::new(),
            expected_translation: String::new(),
            acceptable_translations: vec![],
            semantic_verdict: crate::session::SemanticVerdict::NeedsCorrection,
            errors: vec![],
            per_sentence_feedback: vec![],
            used_vocabulary: uses,
        }
    }

    #[test]
    fn vocabulary_owns_error_when_failed_use_matches() {
        let sentence = sentence_with_uses(vec![
            crate::session::VocabularyUse {
                surface: "pequeño".to_string(),
                lemma: "pequeño".to_string(),
                pos: "ADJ".to_string(),
                side: "student".to_string(),
                spelling_ok: Some(true),
                usage_ok: Some(false),
                ..Default::default()
            },
            crate::session::VocabularyUse {
                surface: "casa".to_string(),
                lemma: "casa".to_string(),
                pos: "NOUN".to_string(),
                side: "target".to_string(),
                ..Default::default()
            },
        ]);
        let owned = GrammarError {
            error_type: crate::session::GrammarErrorType::Major,
            pattern: "pequeño vs pequeña".to_string(),
            explanation: "La casa es pequeña: the adjective must agree in gender.".to_string(),
            ..Default::default()
        };
        assert!(vocabulary_owns_error(&sentence, &owned));
    }

    #[test]
    fn vocabulary_owns_error_rejects_constructions_and_clean_uses() {
        let sentence = sentence_with_uses(vec![crate::session::VocabularyUse {
            surface: "caro".to_string(),
            lemma: "caro".to_string(),
            pos: "ADJ".to_string(),
            side: "student".to_string(),
            spelling_ok: Some(true),
            usage_ok: Some(false),
            ..Default::default()
        }]);
        // A word-pair confusion about different words is not owned.
        let construction = GrammarError {
            error_type: crate::session::GrammarErrorType::Major,
            pattern: "word order".to_string(),
            explanation: "Adjectives usually follow the noun in Spanish.".to_string(),
            ..Default::default()
        };
        assert!(!vocabulary_owns_error(&sentence, &construction));
        // No failed student-side use -> nothing to own.
        let sentence = sentence_with_uses(vec![crate::session::VocabularyUse {
            surface: "pequeño".to_string(),
            lemma: "pequeño".to_string(),
            side: "student".to_string(),
            spelling_ok: Some(true),
            usage_ok: Some(true),
            ..Default::default()
        }]);
        let error = GrammarError {
            pattern: "pequeño vs pequeña".to_string(),
            ..Default::default()
        };
        assert!(!vocabulary_owns_error(&sentence, &error));
    }

    // --- match_warmup_items ---

    fn forced_lemma(id: &str, lemma: &str, translation: &str) -> Lemma {
        Lemma {
            id: id.to_string(),
            lemma: lemma.to_string(),
            pos: "VERB".to_string(),
            translation: translation.to_string(),
            ..Default::default()
        }
    }

    fn raw_warmup(
        lemma: &str,
        translation: Option<&str>,
        example: Option<&str>,
    ) -> crate::llm::parse::RawWarmupItem {
        crate::llm::parse::RawWarmupItem {
            lemma: lemma.to_string(),
            pos: None,
            cefr_level: None,
            translation: translation.map(|s| s.to_string()),
            example: example.map(|s| s.to_string()),
        }
    }

    #[test]
    fn warmup_match_uses_llm_translation_and_example() {
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        let raw = vec![raw_warmup("Comer", Some("кушать"), Some("Como pan."))];
        let items = match_warmup_items(&forced, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma_id.as_deref(), Some("es-comer"));
        // The display form comes from the stored lemma, not the LLM casing.
        assert_eq!(items[0].lemma, "comer");
        assert_eq!(items[0].translation, "кушать");
        assert_eq!(items[0].example.as_deref(), Some("Como pan."));
        assert_eq!(items[0].pos.as_deref(), Some("VERB"));
    }

    #[test]
    fn warmup_match_falls_back_to_stored_translation() {
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        let raw = vec![raw_warmup("comer", None, None)];
        let items = match_warmup_items(&forced, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "есть");
        assert_eq!(items[0].example, None);
    }

    #[test]
    fn warmup_unmatched_lemma_uses_stored_data() {
        let forced = vec![{
            let mut l = forced_lemma("es-comer", "comer", "есть");
            l.cefr_level = Some("A2".to_string());
            l
        }];
        let items = match_warmup_items(&forced, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "есть");
        assert_eq!(items[0].cefr_level.as_deref(), Some("A2"));
        assert_eq!(items[0].example, None);
    }

    #[test]
    fn warmup_skips_lemmas_without_translation() {
        // Unmatched and no stored translation: nothing to teach.
        let forced = vec![
            forced_lemma("es-comer", "comer", "есть"),
            forced_lemma("es-ser", "ser", ""),
        ];
        let items = match_warmup_items(&forced, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");

        // Matched but the LLM returned an empty translation and the stored
        // one is empty too: skipped as well.
        let forced = vec![forced_lemma("es-ser", "ser", "")];
        let raw = vec![raw_warmup("ser", Some(""), Some("Soy yo."))];
        assert!(match_warmup_items(&forced, raw).is_empty());
    }

    #[test]
    fn warmup_drops_raw_items_matching_no_forced_lemma() {
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        let raw = vec![
            raw_warmup("comer", None, None),
            raw_warmup("unrelated", Some("x"), None),
            // Empty lemmas never match anything.
            raw_warmup("", Some("y"), None),
        ];
        let items = match_warmup_items(&forced, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
    }

    #[test]
    fn warmup_preserves_forced_order_and_caps_at_eight() {
        let forced: Vec<Lemma> = (0..10)
            .map(|i| forced_lemma(&format!("es-w{i}"), &format!("w{i}"), "t"))
            .collect();
        // Raw order is the reverse of the forced order; output must still
        // follow the forced order.
        let raw: Vec<_> = (0..10)
            .rev()
            .map(|i| raw_warmup(&format!("w{i}"), None, None))
            .collect();
        let items = match_warmup_items(&forced, raw);
        assert_eq!(items.len(), MAX_WARMUP_ITEMS);
        let lemmas: Vec<&str> = items.iter().map(|i| i.lemma.as_str()).collect();
        assert_eq!(lemmas, ["w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7"]);
    }
}
