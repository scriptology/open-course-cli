//! Serde DTOs of the LLM contract: exercises requested from the model and
//! the analysis it returns. Field names and serde attributes are part of the
//! prompt/response schema — change them together with the prompts.

use serde::{Deserialize, Serialize};

use crate::curriculum::Topic;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Exercise {
    pub id: String,
    pub target_sentence: String,
    pub expected_translation: String,
    #[serde(default)]
    pub acceptable_translations: Vec<String>,
    pub target_topic_ids: Vec<String>,
    pub side_topic_ids: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec_string")]
    pub expected_patterns: Vec<String>,
    pub hint: Option<String>,
}

fn string_or_vec_string<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrVec;

    impl<'de> serde::de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or an array of strings")
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, seq: S) -> std::result::Result<Self::Value, S::Error>
        where
            S: serde::de::SeqAccess<'de>,
        {
            serde::Deserialize::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

/// Whether a warm-up card reinforces a word the learner has already been
/// evaluated on (`Review`) or introduces one they haven't (`New`) — either a
/// word with no `Lemma` row at all, or one that exists but is still
/// `STATUS_NEW` (never evaluated). Both count as "new" to the learner and
/// get the same visual treatment; only words the learner has actually been
/// scored on (`STATUS_PRACTICING`, possibly weak) are `Review`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum WarmupKind {
    #[default]
    Review,
    New,
}

/// One warm-up card shown before the session's exercises: either a forced
/// vocabulary lemma being reinforced, or a genuinely new word previewed from
/// the session's freshly-generated exercises. Filled by matching the LLM's
/// `warmup`/`vocabulary` output against the session's forced lemmas and
/// existing vocabulary (see `vocabulary::match_warmup_items` and
/// `vocabulary::new_word_items`); never persisted.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct WarmupItem {
    /// Id of the lemma this card was built from; `None` for a genuinely new
    /// word that has no `Lemma` row yet.
    #[serde(default)]
    pub lemma_id: Option<String>,
    pub lemma: String,
    /// Universal Dependencies part-of-speech tag ("NOUN", "VERB", ...).
    #[serde(default)]
    pub pos: Option<String>,
    /// Approximate CEFR level ("A1"–"C2").
    #[serde(default)]
    pub cefr_level: Option<String>,
    /// Translation in the learner's native language.
    #[serde(default)]
    pub translation: String,
    /// Short example sentence in the target language.
    #[serde(default)]
    pub example: Option<String>,
    /// `Review` (reinforcing a known-but-weak word) or `New` (previewing a
    /// word the learner hasn't been evaluated on yet).
    #[serde(default)]
    pub kind: WarmupKind,
}

/// One cloze (fill-in-the-blank with a word bank) item shown after the
/// warm-up and before the translation exercises: a simple target-language
/// sentence with the target word blanked out and 3–4 options to choose
/// from. Built by matching the LLM's `cloze` output against the learner's
/// vocabulary (see `vocabulary::cloze_items`) for content words without
/// positive learning progress; never persisted.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClozeItem {
    /// Id of the lemma this item was built from; `None` for a word that has
    /// no `Lemma` row yet (same convention as `WarmupItem`).
    #[serde(default)]
    pub lemma_id: Option<String>,
    pub lemma: String,
    /// Universal Dependencies part-of-speech tag ("NOUN", "VERB", ...).
    #[serde(default)]
    pub pos: Option<String>,
    /// Approximate CEFR level ("A1"–"C2").
    #[serde(default)]
    pub cefr_level: Option<String>,
    /// Full target-language sentence with the target word replaced by a
    /// single `_____` placeholder (inserted by the pipeline, not the LLM).
    pub sentence: String,
    /// The correct word form as it appears in the sentence.
    pub answer: String,
    /// 3–4 choices including `answer`; order is shuffled by the caller.
    pub options: Vec<String>,
    /// Translation of the sentence in the learner's native language
    /// (learner support).
    #[serde(default)]
    pub translation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub session_score: Option<f64>,
    #[serde(default)]
    pub sentences: Vec<SentenceAnalysis>,
    #[serde(default)]
    pub evaluated_topics: Vec<EvaluatedTopic>,
    #[serde(default)]
    pub new_topics: Vec<Topic>,
    /// Word-specific items (e.g. "Adjective: Caro vs Rico") routed to the
    /// learning_items table instead of the curriculum. Filled by the pipeline,
    /// never by the LLM, so it is excluded from serde and the JSON schema.
    #[serde(skip, default)]
    #[schemars(skip)]
    pub new_learning_items: Vec<crate::learning_items::LearningItem>,
    /// Vocabulary entries created or updated while applying the analysis.
    /// Filled by the pipeline, never by the LLM, so they are excluded from
    /// serde and the JSON schema.
    #[serde(skip, default)]
    #[schemars(skip)]
    pub new_lemmas: Vec<crate::vocabulary::Lemma>,
    #[serde(skip, default)]
    #[schemars(skip)]
    pub new_forms: Vec<crate::vocabulary::Form>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SentenceAnalysis {
    pub sentence_number: i32,
    #[serde(default)]
    pub student_translation: String,
    #[serde(default)]
    pub expected_translation: String,
    #[serde(default)]
    pub acceptable_translations: Vec<String>,
    #[serde(default)]
    pub semantic_verdict: SemanticVerdict,
    pub errors: Vec<GrammarError>,
    pub per_sentence_feedback: Vec<FeedbackComment>,
    /// Content words extracted from the expected translation (side "target")
    /// and from the student's translation (side "student", with per-use
    /// assessments). Defaults to empty for older LLM responses.
    #[serde(default)]
    pub used_vocabulary: Vec<VocabularyUse>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct VocabularyUse {
    /// The surface form as it appeared in the sentence.
    #[serde(default)]
    pub surface: String,
    /// Dictionary headword (lemma) of the surface form.
    #[serde(default)]
    pub lemma: String,
    /// Universal Dependencies part-of-speech tag ("NOUN", "VERB", ...).
    #[serde(default)]
    pub pos: String,
    /// UD features in "Attr=Val|..." format, as returned by the LLM.
    #[serde(default)]
    pub feats: String,
    /// "target" — from the expected translation; "student" — from the
    /// student's translation.
    #[serde(default)]
    pub side: String,
    /// Student-side only: whether the surface is spelled acceptably
    /// (missing diacritics/punctuation do NOT count as misspellings).
    #[serde(default)]
    pub spelling_ok: Option<bool>,
    /// Student-side only: whether the word/form is used correctly.
    #[serde(default)]
    pub usage_ok: Option<bool>,
    /// Whether the surface matches a form used in the expected (or an
    /// acceptable) translation. Target-side uses are always expected.
    #[serde(default)]
    pub expected_form: bool,
    /// LLM estimate of the lemma's approximate CEFR level ("A1"–"C2");
    /// `None` when the model omits it.
    #[serde(default)]
    pub cefr_level: Option<String>,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum SemanticVerdict {
    #[default]
    Correct,
    Acceptable,
    NeedsCorrection,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FeedbackComment {
    pub comment: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GrammarError {
    #[serde(rename = "type", default)]
    pub error_type: GrammarErrorType,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub explanation: String,
    #[serde(default)]
    pub topic_ids: Vec<String>,
    #[serde(default)]
    pub new_topics: Vec<NewTopicRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct NewTopicRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub level: Option<String>,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default,
)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum GrammarErrorType {
    Critical,
    Major,
    Minor,
    #[default]
    Spelling,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedTopic {
    pub topic_id: String,
    pub score: f64,
    #[serde(default)]
    pub previous_score: Option<f64>,
}
