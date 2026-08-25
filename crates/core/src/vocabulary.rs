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
///
/// Conjunctions (SCONJ/CCONJ) count as content: discourse markers like
/// "although", "because", "however", "therefore" are a small, semantically
/// rich closed class students genuinely need to learn word by word — no
/// different from the conjunctive adverbs ("however", "moreover") already
/// tracked here as ADV. Excluding them meant a topic like "Discourse
/// Markers and Connectors" could track literally none of its own target
/// vocabulary, since most of it is SCONJ/CCONJ.
pub const CONTENT_POS: &[&str] = &["NOUN", "VERB", "ADJ", "ADV", "PROPN", "SCONJ", "CCONJ"];

/// Function-word POS tags (the closed UD set) excluded from forced
/// vocabulary practice: purely grammatical scaffolding (prepositions,
/// determiners, pronouns, auxiliaries, particles, interjections,
/// punctuation, symbols) rather than words with independent lexical
/// meaning.
const FUNCTION_POS: &[&str] = &[
    "ADP", "AUX", "DET", "INTJ", "PART", "PRON", "PUNCT", "SYM", "X",
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

/// Normalized word tokens of every exercise's expected and acceptable
/// translations: the target-language sentences where the taught words
/// actually appear (`target_sentence` is in the learner's native language,
/// so words from it would never match). Each sentence is split on
/// non-alphanumeric characters and each token is folded through
/// `normalize_key`.
fn exercise_tokens(exercises: &[crate::session::Exercise]) -> HashSet<String> {
    exercises
        .iter()
        .flat_map(|ex| {
            std::iter::once(ex.expected_translation.as_str())
                .chain(ex.acceptable_translations.iter().map(String::as_str))
        })
        .flat_map(|sentence| sentence.split(|c: char| !c.is_alphanumeric()))
        .map(normalize_key)
        .filter(|token| !token.is_empty())
        .collect()
}

/// Builds warm-up cards for a session's forced lemmas from the LLM's raw
/// `warmup` output. Only words that actually appear in the session's
/// exercises get a card: a forced lemma qualifies when its headword or one
/// of its known forms (matched via `normalize_key` against the tokens of the
/// exercises' expected/acceptable translations) shows up in a target-language
/// sentence. A lemma with no known forms is also included when the LLM
/// returned a warm-up entry for it (a new word whose inflections cannot be
/// verified yet). Cards follow the forced-lemma order and are not capped:
/// every word the learner has never answered correctly gets a card. A raw
/// entry whose lemma matches supplies the translation and example; otherwise
/// the stored translation is used with no example. Lemmas without any
/// translation are skipped (there is nothing to teach), and raw entries
/// matching no forced lemma are dropped. Each card is tagged
/// `WarmupKind::New` when the lemma hasn't been evaluated yet (`STATUS_NEW`)
/// or `WarmupKind::Review` otherwise (see `new_word_items` for words the
/// learner has never answered correctly, tagged `New` the same way).
pub fn match_warmup_items(
    forced: &[Lemma],
    forms: &[Form],
    exercises: &[crate::session::Exercise],
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

    let tokens = exercise_tokens(exercises);
    let mut forms_by_lemma: HashMap<&str, Vec<&Form>> = HashMap::new();
    for form in forms {
        forms_by_lemma
            .entry(form.lemma_id.as_str())
            .or_default()
            .push(form);
    }

    forced
        .iter()
        .filter(|lemma| {
            let key = normalize_key(&lemma.lemma);
            let known_forms = forms_by_lemma.get(lemma.id.as_str());
            let appears = tokens.contains(&key)
                || known_forms.is_some_and(|forms| {
                    forms
                        .iter()
                        .any(|f| tokens.contains(&normalize_key(&f.surface)))
                });
            // A form-less lemma cannot be verified against the exercises, so
            // an LLM warm-up entry is trusted on its own.
            appears
                || (known_forms.is_none_or(|forms| forms.is_empty()) && by_key.contains_key(&key))
        })
        .filter_map(|lemma| {
            let key = normalize_key(&lemma.lemma);
            // A lemma the learner hasn't been evaluated on yet (STATUS_NEW,
            // mastery 0) counts as "new" to them, same as a word with no
            // Lemma row at all (see `new_word_items`) — only a word already
            // scored at least once (STATUS_PRACTICING) is a plain review.
            let kind = if lemma.status == STATUS_NEW {
                crate::session::WarmupKind::New
            } else {
                crate::session::WarmupKind::Review
            };
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
                        kind,
                    }
                }
                None => crate::session::WarmupItem {
                    lemma_id: Some(lemma.id.clone()),
                    lemma: lemma.lemma.clone(),
                    pos: non_empty(Some(lemma.pos.clone())),
                    cefr_level: lemma.cefr_level.clone(),
                    translation: lemma.translation.clone(),
                    example: None,
                    kind,
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

/// Builds warm-up preview cards for words the learner does not know yet:
/// content words the LLM reports using in the session's exercises (`raw`,
/// the `"vocabulary"` array from `Exercises`) that either have no `Lemma`
/// row at all in `existing_lemmas` or whose existing row has
/// `correct_uses == 0` — never answered correctly, which covers both
/// `STATUS_NEW` and a word the learner has only ever gotten wrong. A word
/// with `correct_uses > 0` counts as known and gets no card. Same invariants
/// as `match_warmup_items`: the word must actually appear in a generated
/// exercise's target-language sentence (no hallucinated vocabulary) —
/// checked against `surface` (the inflected form actually used), not
/// `lemma` itself, since an inflected verb's dictionary headword usually
/// never appears verbatim (falls back to `lemma` when the LLM omits
/// `surface`) — and non-content POS and empty translations are skipped.
/// For a word with an existing row the card carries that row's id and falls
/// back to the stored `pos`/`cefr_level`/`translation` when the raw item's
/// are empty. Every card is tagged `WarmupKind::New`.
pub fn new_word_items(
    existing_lemmas: &[Lemma],
    exercises: &[crate::session::Exercise],
    raw: Vec<crate::llm::parse::RawVocabularyItem>,
) -> Vec<crate::session::WarmupItem> {
    use std::collections::HashMap;

    let by_key: HashMap<String, &Lemma> = existing_lemmas
        .iter()
        .map(|lemma| (normalize_key(&lemma.lemma), lemma))
        .collect();
    let tokens = exercise_tokens(exercises);

    let mut seen_keys: HashSet<String> = HashSet::new();
    raw.into_iter()
        .filter_map(|item| {
            let key = normalize_key(item.lemma.trim());
            if key.is_empty() || !seen_keys.insert(key.clone()) {
                return None;
            }
            let existing = by_key.get(&key).copied();
            if existing.is_some_and(|lemma| lemma.correct_uses > 0) {
                return None;
            }
            let pos = item.pos.clone().unwrap_or_default();
            if !is_content_pos(&pos) {
                return None;
            }
            let surface_key = normalize_key(item.surface.trim());
            let appears = if surface_key.is_empty() {
                tokens.contains(&key)
            } else {
                tokens.contains(&surface_key)
            };
            if !appears {
                return None;
            }
            let translation = non_empty(item.translation).or_else(|| {
                existing.and_then(|lemma| non_empty(Some(lemma.translation.clone())))
            })?;
            Some(crate::session::WarmupItem {
                lemma_id: existing.map(|lemma| lemma.id.clone()),
                lemma: item.lemma,
                pos: non_empty(item.pos)
                    .or_else(|| existing.and_then(|lemma| non_empty(Some(lemma.pos.clone())))),
                cefr_level: non_empty(item.cefr_level)
                    .or_else(|| existing.and_then(|lemma| lemma.cefr_level.clone())),
                translation,
                example: None,
                kind: crate::session::WarmupKind::New,
            })
        })
        .collect()
}

/// Placeholder replacing the answer in a cloze sentence. Inserted by
/// `cloze_items` at the real char boundaries of the matched token, never by
/// the LLM, so every item shares the exact same marker.
pub const CLOZE_BLANK: &str = "_____";

/// Replaces the first word token of `sentence` whose normalized form equals
/// `answer_key` with `CLOZE_BLANK`, returning the rewritten sentence.
/// Matching is case- and Unicode-normalized (via `normalize_key`) but the
/// replacement happens at the real char boundaries of the original text.
/// `None` when the answer does not appear as a token — the item must be
/// dropped as a probable hallucination.
fn blank_answer(sentence: &str, answer_key: &str) -> Option<String> {
    let mut token_start: Option<usize> = None;
    let check = |start: usize, end: usize| {
        (normalize_key(&sentence[start..end]) == answer_key)
            .then(|| format!("{}{}{}", &sentence[..start], CLOZE_BLANK, &sentence[end..]))
    };
    for (i, c) in sentence.char_indices() {
        if c.is_alphanumeric() {
            if token_start.is_none() {
                token_start = Some(i);
            }
        } else if let Some(start) = token_start.take()
            && let Some(blanked) = check(start, i)
        {
            return Some(blanked);
        }
    }
    if let Some(start) = token_start {
        return check(start, sentence.len());
    }
    None
}

/// Builds cloze (word-bank) items for a session from the LLM's raw `cloze`
/// output (the `"cloze"` array from `Exercises`). Only words without
/// positive learning progress get an item: those with no `Lemma` row at all
/// (matched by `normalize_key(lemma)`, same as `new_word_items`) or whose
/// existing row has `correct_uses == 0`; forced-vocabulary lemmas count as
/// existing rows too, so a forced lemma with `correct_uses == 0` is kept and
/// linked even when it is absent from `existing_lemmas`. Anti-hallucination
/// validation: the `answer` (compared via `normalize_key`) must appear in
/// `sentence`; the first matching token is replaced with `CLOZE_BLANK` at
/// the real char boundaries, so the placeholder is guaranteed consistent —
/// the item is dropped otherwise. `options` is the answer plus distractors,
/// deduped via `normalize_key`; items that do not end up with 3–4 unique
/// options are dropped. Items are deduped by `normalize_key(lemma)` (first
/// wins). For a word with an existing row the item carries that row's id
/// and falls back to the stored `pos`/`cefr_level` when the raw item's are
/// empty.
pub fn cloze_items(
    existing_lemmas: &[Lemma],
    forced_vocabulary: &[Lemma],
    raw: Vec<crate::llm::parse::RawClozeItem>,
) -> Vec<crate::session::ClozeItem> {
    use std::collections::HashMap;

    let mut by_key: HashMap<String, &Lemma> = HashMap::new();
    for lemma in forced_vocabulary {
        by_key.insert(normalize_key(&lemma.lemma), lemma);
    }
    // Existing rows win over forced duplicates: they are the fuller record.
    for lemma in existing_lemmas {
        by_key.insert(normalize_key(&lemma.lemma), lemma);
    }

    let mut seen_keys: HashSet<String> = HashSet::new();
    raw.into_iter()
        .filter_map(|item| {
            let key = normalize_key(item.lemma.trim());
            if key.is_empty() || !seen_keys.insert(key.clone()) {
                return None;
            }
            let existing = by_key.get(&key).copied();
            if existing.is_some_and(|lemma| lemma.correct_uses > 0) {
                return None;
            }
            let answer = item.answer.trim();
            let answer_key = normalize_key(answer);
            if answer_key.is_empty() {
                return None;
            }
            let sentence = blank_answer(item.sentence.trim(), &answer_key)?;
            let mut options: Vec<String> = Vec::new();
            let mut option_keys: HashSet<String> = HashSet::new();
            options.push(answer.to_string());
            option_keys.insert(answer_key);
            for distractor in &item.distractors {
                let distractor = distractor.trim();
                let distractor_key = normalize_key(distractor);
                if distractor_key.is_empty() || !option_keys.insert(distractor_key) {
                    continue;
                }
                options.push(distractor.to_string());
            }
            if !(3..=4).contains(&options.len()) {
                return None;
            }
            Some(crate::session::ClozeItem {
                lemma_id: existing.map(|lemma| lemma.id.clone()),
                lemma: item.lemma,
                pos: non_empty(item.pos)
                    .or_else(|| existing.and_then(|lemma| non_empty(Some(lemma.pos.clone())))),
                cefr_level: non_empty(item.cefr_level)
                    .or_else(|| existing.and_then(|lemma| lemma.cefr_level.clone())),
                sentence,
                answer: answer.to_string(),
                options,
                translation: non_empty(item.translation).unwrap_or_default(),
            })
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

/// Vocabulary status derived from mastery and practice history: 0 when the
/// word has never been practiced, 1 from a mastery of 50 or once the word has
/// been contacted at least once (`practiced`), 2 from a mastery of 80.
///
/// `new` = never appeared in the student's output; `practicing` = has been
/// seen/used (even with an error) but not yet mastered; `known` = mastered.
/// An observed error caps the status at `STATUS_PRACTICING`: a mistake means
/// the word is not fully known yet, regardless of mastery.
pub fn derive_status(mastery: f64, had_error: bool, practiced: bool) -> i32 {
    let base = if mastery >= COMPLETED_THRESHOLD {
        STATUS_KNOWN
    } else if mastery >= MASTERY_THRESHOLD || practiced {
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
        // Conjunctions count as content: discourse markers ("although",
        // "however", "therefore") are lexical items worth tracking, not
        // grammatical scaffolding.
        for pos in [
            "NOUN", "verb", "Adj", "ADV", "PROPN", "SCONJ", "CCONJ", "sconj",
        ] {
            assert!(is_content_pos(pos), "{pos} should be content");
        }
        for pos in ["PART", "ADP", "DET", "PRON", "AUX", "PUNCT"] {
            assert!(!is_content_pos(pos), "{pos} should be function");
        }
        // Empty or unrecognized tags are lazily accepted.
        assert!(is_content_pos(""));
        assert!(is_content_pos("NUM"));
        assert!(is_content_pos("garbage"));
    }

    #[test]
    fn derive_status_thresholds() {
        assert_eq!(derive_status(0.0, false, false), STATUS_NEW);
        assert_eq!(derive_status(49.9, false, false), STATUS_NEW);
        assert_eq!(derive_status(50.0, false, false), STATUS_PRACTICING);
        assert_eq!(derive_status(79.9, false, false), STATUS_PRACTICING);
        assert_eq!(derive_status(80.0, false, false), STATUS_KNOWN);
        assert_eq!(derive_status(100.0, false, false), STATUS_KNOWN);

        assert_eq!(derive_status(34.0, false, true), STATUS_PRACTICING);
        assert_eq!(derive_status(0.0, false, true), STATUS_PRACTICING);
    }

    #[test]
    fn derive_status_error_caps_at_practicing() {
        assert_eq!(derive_status(95.0, true, false), STATUS_PRACTICING);
        assert_eq!(derive_status(60.0, true, false), STATUS_PRACTICING);
        assert_eq!(derive_status(34.0, true, true), STATUS_PRACTICING);
        assert_eq!(derive_status(0.0, true, true), STATUS_PRACTICING);
        assert_eq!(derive_status(10.0, true, false), STATUS_NEW);
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

    /// Exercise whose expected (target-language) translation contains the
    /// given sentence; the native-language `target_sentence` is irrelevant
    /// for warm-up matching.
    fn exercise(expected_translation: &str) -> crate::session::Exercise {
        crate::session::Exercise {
            id: "ex1".to_string(),
            target_sentence: String::new(),
            expected_translation: expected_translation.to_string(),
            acceptable_translations: vec![],
            target_topic_ids: vec![],
            side_topic_ids: vec![],
            expected_patterns: vec![],
            hint: None,
        }
    }

    fn known_form(lemma_id: &str, surface: &str) -> Form {
        Form {
            id: Form::id(lemma_id, surface),
            lemma_id: lemma_id.to_string(),
            surface: surface.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn warmup_match_uses_llm_translation_and_example() {
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        let raw = vec![raw_warmup("Comer", Some("кушать"), Some("Como pan."))];
        let items = match_warmup_items(&forced, &[], &[], raw);
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
        let items = match_warmup_items(&forced, &[], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "есть");
        assert_eq!(items[0].example, None);
    }

    #[test]
    fn warmup_lemma_in_exercise_without_llm_entry_uses_stored_data() {
        let forced = vec![{
            let mut l = forced_lemma("es-comer", "comer", "есть");
            l.cefr_level = Some("A2".to_string());
            l
        }];
        let exercises = vec![exercise("Quiero comer pan.")];
        let items = match_warmup_items(&forced, &[], &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "есть");
        assert_eq!(items[0].cefr_level.as_deref(), Some("A2"));
        assert_eq!(items[0].example, None);
    }

    #[test]
    fn warmup_excludes_lemma_absent_from_exercises_without_llm_entry() {
        let forced = vec![
            forced_lemma("es-comer", "comer", "есть"),
            forced_lemma("es-beber", "beber", "пить"),
        ];
        // Only "comer" shows up in the exercises; "beber" has no known
        // forms and no LLM entry, so it gets no card.
        let exercises = vec![exercise("Quiero comer pan.")];
        let items = match_warmup_items(&forced, &[], &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");

        // Known forms that also never appear do not rescue the lemma.
        let forms = vec![known_form("es-beber", "bebo")];
        let items = match_warmup_items(&forced, &forms, &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
    }

    #[test]
    fn warmup_matches_inflected_form_in_exercise() {
        let forced = vec![forced_lemma("es-hablar", "hablar", "говорить")];
        let forms = vec![known_form("es-hablar", "hablo")];
        let exercises = vec![exercise("Hablo con Maria.")];
        let items = match_warmup_items(&forced, &forms, &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "hablar");
        assert_eq!(items[0].translation, "говорить");
    }

    #[test]
    fn warmup_matching_is_case_insensitive_but_keeps_diacritics() {
        let forced = vec![
            forced_lemma("es-comer", "Comer", "есть"),
            forced_lemma("es-ano", "ano", "год (без тильды)"),
        ];
        // "COMER" matches case-insensitively; "año" must not match "ano".
        let exercises = vec![exercise("COMER pan este año.")];
        let items = match_warmup_items(&forced, &[], &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "Comer");
    }

    #[test]
    fn warmup_formless_lemma_with_llm_entry_is_trusted() {
        let forced = vec![
            forced_lemma("es-comer", "comer", "есть"),
            forced_lemma("es-hablar", "hablar", "говорить"),
        ];
        // "hablar" has a known form that never appears, so its LLM entry is
        // not enough; "comer" has no forms and is trusted.
        let forms = vec![known_form("es-hablar", "hablo")];
        let raw = vec![
            raw_warmup("comer", Some("кушать"), None),
            raw_warmup("hablar", Some("разговаривать"), None),
        ];
        let items = match_warmup_items(&forced, &forms, &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
        assert_eq!(items[0].translation, "кушать");
    }

    #[test]
    fn warmup_skips_lemmas_without_translation() {
        // Present in an exercise but with no stored translation: nothing to
        // teach.
        let forced = vec![
            forced_lemma("es-comer", "comer", "есть"),
            forced_lemma("es-ser", "ser", ""),
        ];
        let exercises = vec![exercise("Quiero comer y ser feliz.")];
        let items = match_warmup_items(&forced, &[], &exercises, vec![]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");

        // Matched but the LLM returned an empty translation and the stored
        // one is empty too: skipped as well.
        let forced = vec![forced_lemma("es-ser", "ser", "")];
        let raw = vec![raw_warmup("ser", Some(""), Some("Soy yo."))];
        assert!(match_warmup_items(&forced, &[], &[], raw).is_empty());
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
        let items = match_warmup_items(&forced, &[], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
    }

    #[test]
    fn warmup_preserves_forced_order_without_cap() {
        let forced: Vec<Lemma> = (0..10)
            .map(|i| forced_lemma(&format!("es-w{i}"), &format!("w{i}"), "t"))
            .collect();
        // Raw order is the reverse of the forced order; output must still
        // follow the forced order, and every qualifying lemma gets a card
        // (no cap).
        let raw: Vec<_> = (0..10)
            .rev()
            .map(|i| raw_warmup(&format!("w{i}"), None, None))
            .collect();
        let items = match_warmup_items(&forced, &[], &[], raw);
        let lemmas: Vec<&str> = items.iter().map(|i| i.lemma.as_str()).collect();
        assert_eq!(
            lemmas,
            ["w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8", "w9"]
        );
    }

    #[test]
    fn warmup_skips_untranslated_lemmas_without_cap() {
        // Ten qualifying lemmas, the first without a translation: it is
        // skipped and all nine translated lemmas get cards.
        let mut forced = vec![forced_lemma("es-w0", "w0", "")];
        forced.extend((1..10).map(|i| forced_lemma(&format!("es-w{i}"), &format!("w{i}"), "t")));
        let raw: Vec<_> = (0..10)
            .map(|i| raw_warmup(&format!("w{i}"), None, None))
            .collect();
        let items = match_warmup_items(&forced, &[], &[], raw);
        assert_eq!(items.len(), 9);
        assert_eq!(items[0].lemma, "w1");
        assert_eq!(items[8].lemma, "w9");
    }

    #[test]
    fn match_warmup_items_tags_new_status_as_new() {
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        // forced_lemma defaults status to STATUS_NEW (0).
        let items = match_warmup_items(&forced, &[], &[], vec![raw_warmup("comer", None, None)]);
        assert_eq!(items[0].kind, crate::session::WarmupKind::New);
    }

    #[test]
    fn match_warmup_items_tags_practicing_status_as_review() {
        let mut lemma = forced_lemma("es-comer", "comer", "есть");
        lemma.status = STATUS_PRACTICING;
        lemma.mastery = 20.0;
        let items = match_warmup_items(&[lemma], &[], &[], vec![raw_warmup("comer", None, None)]);
        assert_eq!(items[0].kind, crate::session::WarmupKind::Review);
    }

    // --- new_word_items ---

    fn raw_vocabulary(
        lemma: &str,
        surface: &str,
        pos: &str,
        translation: Option<&str>,
    ) -> crate::llm::parse::RawVocabularyItem {
        crate::llm::parse::RawVocabularyItem {
            lemma: lemma.to_string(),
            surface: surface.to_string(),
            pos: Some(pos.to_string()),
            cefr_level: None,
            translation: translation.map(|s| s.to_string()),
        }
    }

    #[test]
    fn new_word_items_excludes_lemmas_answered_correctly() {
        let mut known = forced_lemma("es-comer", "comer", "есть");
        known.correct_uses = 3;
        let existing = vec![known];
        // "comer" appears via its exact headword form here, so the surface
        // check wouldn't be what excludes it — the correct_uses check is.
        let exercises = vec![exercise("Quiero comer y beber.")];
        let raw = vec![
            raw_vocabulary("comer", "comer", "VERB", Some("кушать")),
            raw_vocabulary("beber", "beber", "VERB", Some("пить")),
        ];
        let items = new_word_items(&existing, &exercises, raw);
        // "comer" was answered correctly before — known, no card; only
        // "beber" (no Lemma row at all) qualifies.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "beber");
        assert_eq!(items[0].lemma_id, None);
        assert_eq!(items[0].kind, crate::session::WarmupKind::New);
    }

    #[test]
    fn new_word_items_includes_existing_lemma_never_answered_correctly() {
        // A Lemma row exists but the word was never answered correctly
        // (correct_uses == 0) — whether STATUS_NEW or PRACTICING with only
        // errors — so it still gets a warm-up card, now linked to the row.
        let mut never_used = forced_lemma("es-comer", "comer", "");
        never_used.cefr_level = Some("A1".to_string());
        let mut only_errors = forced_lemma("es-beber", "beber", "пить");
        only_errors.status = STATUS_PRACTICING;
        only_errors.incorrect_uses = 2;
        let existing = vec![never_used, only_errors];
        let exercises = vec![exercise("Quiero comer y beber.")];
        let raw = vec![
            // Raw translation empty: falls back to the stored one — but
            // "comer" has none stored either, so it is still skipped.
            raw_vocabulary("comer", "comer", "VERB", None),
            // Raw pos/cefr/translation present: used verbatim.
            raw_vocabulary("beber", "beber", "VERB", Some("выпивать")),
        ];
        let items = new_word_items(&existing, &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "beber");
        assert_eq!(items[0].lemma_id.as_deref(), Some("es-beber"));
        assert_eq!(items[0].translation, "выпивать");
        assert_eq!(items[0].kind, crate::session::WarmupKind::New);

        // Stored fields fill gaps in the raw item.
        let raw = vec![crate::llm::parse::RawVocabularyItem {
            lemma: "beber".to_string(),
            surface: "beber".to_string(),
            pos: None,
            cefr_level: None,
            translation: None,
        }];
        let items = new_word_items(&existing, &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma_id.as_deref(), Some("es-beber"));
        assert_eq!(items[0].pos.as_deref(), Some("VERB"));
        assert_eq!(items[0].translation, "пить");
    }

    #[test]
    fn new_word_items_matches_inflected_surface_not_headword() {
        // The dictionary headword "comer" never appears verbatim — only its
        // inflected surface "como" does. Matching must go through `surface`.
        let exercises = vec![exercise("Como pan cada día.")];
        let raw = vec![raw_vocabulary("comer", "como", "VERB", Some("кушать"))];
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
    }

    #[test]
    fn new_word_items_falls_back_to_lemma_when_surface_missing() {
        let exercises = vec![exercise("Quiero comer pan.")];
        let raw = vec![raw_vocabulary("comer", "", "VERB", Some("кушать"))];
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn new_word_items_excludes_function_pos() {
        let exercises = vec![exercise("Quiero comer con ella.")];
        let raw = vec![
            raw_vocabulary("comer", "comer", "VERB", Some("кушать")),
            raw_vocabulary("con", "con", "ADP", Some("с")),
        ];
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "comer");
    }

    #[test]
    fn new_word_items_tracks_discourse_marker_conjunctions() {
        // Regression: "although" (SCONJ) must be previewable as a new word,
        // same as any noun/verb — only truly grammatical POS (ADP, DET, ...)
        // are excluded.
        let exercises = vec![exercise("Aunque llueve, salgo.")];
        let raw = vec![raw_vocabulary("aunque", "aunque", "SCONJ", Some("хотя"))];
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "aunque");
    }

    #[test]
    fn new_word_items_requires_translation() {
        let exercises = vec![exercise("Quiero comer pan.")];
        let raw = vec![raw_vocabulary("comer", "comer", "VERB", None)];
        assert!(new_word_items(&[], &exercises, raw).is_empty());
    }

    #[test]
    fn new_word_items_requires_appearance_in_exercises() {
        // The LLM claims "beber" but it never made it into any sentence —
        // same anti-hallucination guard as match_warmup_items.
        let exercises = vec![exercise("Quiero comer pan.")];
        let raw = vec![raw_vocabulary("beber", "bebo", "VERB", Some("пить"))];
        assert!(new_word_items(&[], &exercises, raw).is_empty());
    }

    #[test]
    fn new_word_items_dedups_repeated_lemmas() {
        let exercises = vec![exercise("Como pan y como fruta.")];
        let raw = vec![
            raw_vocabulary("comer", "como", "VERB", Some("кушать")),
            raw_vocabulary("comer", "como", "VERB", Some("есть (дубль)")),
        ];
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "кушать");
    }

    #[test]
    fn appearance_check_matches_translations_not_target_sentence() {
        // "bread" appears only in the native-language target_sentence: no
        // card. "como" appears in the expected translation: card. "fruta"
        // appears only in an acceptable alternative: card too.
        let exercises = vec![crate::session::Exercise {
            target_sentence: "I eat bread.".to_string(),
            ..exercise("Como pan.")
        }];
        let mut with_acceptable = exercises.clone();
        with_acceptable[0].acceptable_translations = vec!["Yo como fruta.".to_string()];
        let raw = vec![
            raw_vocabulary("bread", "bread", "NOUN", Some("хлеб")),
            raw_vocabulary("comer", "como", "VERB", Some("кушать")),
            raw_vocabulary("fruta", "fruta", "NOUN", Some("фрукт")),
        ];
        let items = new_word_items(&[], &with_acceptable, raw);
        let lemmas: Vec<&str> = items.iter().map(|i| i.lemma.as_str()).collect();
        assert_eq!(lemmas, ["comer", "fruta"]);
    }

    #[test]
    fn new_word_items_are_not_capped() {
        let exercises = vec![exercise("w0 w1 w2 w3 w4 w5 w6 w7 w8 w9 w10 w11.")];
        let raw: Vec<_> = (0..12)
            .map(|i| raw_vocabulary(&format!("w{i}"), &format!("w{i}"), "NOUN", Some("t")))
            .collect();
        let items = new_word_items(&[], &exercises, raw);
        assert_eq!(items.len(), 12);
    }

    // --- cloze_items ---

    fn raw_cloze(
        lemma: &str,
        sentence: &str,
        answer: &str,
        distractors: &[&str],
    ) -> crate::llm::parse::RawClozeItem {
        crate::llm::parse::RawClozeItem {
            lemma: lemma.to_string(),
            sentence: sentence.to_string(),
            answer: answer.to_string(),
            distractors: distractors.iter().map(|s| s.to_string()).collect(),
            translation: Some("t".to_string()),
            pos: None,
            cefr_level: None,
        }
    }

    #[test]
    fn cloze_items_excludes_lemmas_answered_correctly() {
        let mut known = forced_lemma("es-comer", "comer", "есть");
        known.correct_uses = 3;
        let existing = vec![known];
        let raw = vec![
            raw_cloze("comer", "Como pan.", "Como", &["Comes", "Comen"]),
            raw_cloze("beber", "Bebo agua.", "Bebo", &["Bebes", "Beber"]),
        ];
        let items = cloze_items(&existing, &[], raw);
        // "comer" was answered correctly before — no item; "beber" has no
        // Lemma row at all and qualifies.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma, "beber");
        assert_eq!(items[0].lemma_id, None);
    }

    #[test]
    fn cloze_items_links_existing_lemma_never_answered_correctly() {
        let mut only_errors = forced_lemma("es-comer", "comer", "есть");
        only_errors.status = STATUS_PRACTICING;
        only_errors.incorrect_uses = 2;
        only_errors.cefr_level = Some("A1".to_string());
        let raw = vec![raw_cloze("Comer", "Como pan.", "Como", &["Comes", "Comen"])];
        let items = cloze_items(&[only_errors], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma_id.as_deref(), Some("es-comer"));
        // Stored pos/cefr fill the raw item's gaps.
        assert_eq!(items[0].pos.as_deref(), Some("VERB"));
        assert_eq!(items[0].cefr_level.as_deref(), Some("A1"));
        // The display form comes from the raw item, not the stored lemma.
        assert_eq!(items[0].lemma, "Comer");
    }

    #[test]
    fn cloze_items_keeps_forced_lemma_with_zero_correct_uses() {
        // A forced lemma whose row is not in `existing_lemmas` still counts
        // as an existing row: kept while correct_uses == 0, dropped once it
        // has positive progress.
        let forced = vec![forced_lemma("es-comer", "comer", "есть")];
        let items = cloze_items(
            &[],
            &forced,
            vec![raw_cloze("comer", "Como pan.", "Como", &["Comes", "Comen"])],
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lemma_id.as_deref(), Some("es-comer"));

        let mut practiced = forced_lemma("es-comer", "comer", "есть");
        practiced.correct_uses = 1;
        let items = cloze_items(
            &[],
            &[practiced],
            vec![raw_cloze("comer", "Como pan.", "Como", &["Comes", "Comen"])],
        );
        assert!(items.is_empty());
    }

    #[test]
    fn cloze_items_drops_answer_absent_from_sentence() {
        // The LLM claims "comes" but the sentence says "como" — probable
        // hallucination, dropped.
        let raw = vec![raw_cloze("comer", "Como pan.", "Comes", &["Comen", "Comer"])];
        assert!(cloze_items(&[], &[], raw).is_empty());
    }

    #[test]
    fn cloze_items_blanks_answer_case_insensitively_at_real_boundaries() {
        // Answer casing differs from the sentence; the match is normalized,
        // the replacement keeps the rest of the sentence untouched.
        let raw = vec![raw_cloze("comer", "Yo como pan cada día.", "Como", &["comes", "comen"])];
        let items = cloze_items(&[], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sentence, "Yo _____ pan cada día.");
        assert_eq!(items[0].answer, "Como");
    }

    #[test]
    fn cloze_items_options_dedup_and_count() {
        // A distractor duplicating the answer (different case) is deduped;
        // two remaining distractors give exactly 3 options.
        let raw = vec![raw_cloze(
            "comer",
            "Como pan.",
            "Como",
            &["como", "Comes", "Comen"],
        )];
        let items = cloze_items(&[], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].options, ["Como", "Comes", "Comen"]);

        // Fewer than 3 unique options: dropped.
        let raw = vec![raw_cloze("comer", "Como pan.", "Como", &["como"])];
        assert!(cloze_items(&[], &[], raw).is_empty());

        // More than 4 unique options: contract violation, dropped.
        let raw = vec![raw_cloze(
            "comer",
            "Como pan.",
            "Como",
            &["comes", "comen", "comer", "comed"],
        )];
        assert!(cloze_items(&[], &[], raw).is_empty());
    }

    #[test]
    fn cloze_items_dedups_repeated_lemmas() {
        let raw = vec![
            raw_cloze("Comer", "Como pan.", "Como", &["Comes", "Comen"]),
            raw_cloze("comer", "Como fruta.", "Como", &["Comes", "Comen"]),
        ];
        let items = cloze_items(&[], &[], raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sentence, "_____ pan.");
    }
}
