//! Serde DTOs of the LLM contract: exercises requested from the model and
//! the analysis it returns. Field names and serde attributes are part of the
//! prompt/response schema — change them together with the prompts.

use serde::{Deserialize, Serialize};

use crate::db::curriculum::Topic;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
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
    pub new_learning_items: Vec<crate::db::learning_items::LearningItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SemanticVerdict {
    #[default]
    Correct,
    Acceptable,
    NeedsCorrection,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackComment {
    pub comment: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
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
#[serde(rename_all = "lowercase")]
pub enum GrammarErrorType {
    Critical,
    Major,
    Minor,
    #[default]
    Spelling,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedTopic {
    pub topic_id: String,
    pub score: f64,
    #[serde(default)]
    pub previous_score: Option<f64>,
}
