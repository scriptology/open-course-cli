use std::path::Path;

use futures_util::future::join_all;
use tokio::sync::mpsc;

use crate::LlmResult;
use crate::client::LlmClient;
use crate::debug_log::{log_debug_event, log_raw_response};
use crate::parse::clean_json_response;
use crate::prompts::{build_new_topic_metadata_prompt, build_topic_metadata_prompt};
use crate::transport::{stream_or_prompt, with_timeout_secs};
use open_course_config::profile::UserProfile;
use open_course_core::curriculum::{Topic, is_abstract_topic_name};
use open_course_core::error::{AppError, Result};
use open_course_core::session::{AnalysisResult, NewTopicRef, unique_topic_ids};

const MAX_TOKENS_TOPIC_METADATA: u32 = 4096;

/// Fill empty topic fields with profile defaults after parsing LLM output.
fn normalize_topic_defaults(topic: &mut Topic, profile: &UserProfile) {
    if topic.target_lang.is_empty() {
        topic.target_lang = profile.target_language.clone();
    }
    if topic.native_lang.is_empty() {
        topic.native_lang = profile.native_language.clone();
    }
    if topic.version == 0 {
        topic.version = 1;
    }
    if topic.level.is_none() {
        topic.level = open_course_core::curriculum::difficulty_to_cefr(&topic.difficulty);
    }
    if topic.order.is_none() {
        topic.order = Some(topic.cefr_numeric() * 1000);
    }
}

async fn generate_new_topic(
    client: &dyn LlmClient,
    profile: &UserProfile,
    new_topic: &NewTopicRef,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Topic> {
    let prompt = build_new_topic_metadata_prompt(profile, new_topic);
    let system = "You are a curriculum design assistant. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let response = with_timeout_secs(
        stream_or_prompt(
            client,
            &prompt,
            system,
            stream_tx,
            "Generating new topic...",
            None,
            MAX_TOKENS_TOPIC_METADATA,
        ),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("New topic generation failed: {e}")))?;

    log_raw_response(&prompt, &response.raw, "new-topic", data_dir);

    let cleaned = clean_json_response(&response.raw);
    let mut topic: Topic = serde_json::from_str(&cleaned).map_err(|e| {
        AppError::Llm(format!(
            "Failed to parse new topic response: {e}; raw: {}",
            response.raw
        ))
    })?;

    normalize_topic_defaults(&mut topic, profile);

    Ok(topic)
}

pub async fn finalize_analysis_with_new_topics(
    client: &dyn LlmClient,
    profile: &UserProfile,
    existing_topics: &[Topic],
    mut analysis: AnalysisResult,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<AnalysisResult> {
    let mut seen: std::collections::HashMap<String, NewTopicRef> = std::collections::HashMap::new();
    for sentence in &analysis.sentences {
        for error in &sentence.errors {
            if error.error_type == open_course_core::session::GrammarErrorType::Spelling {
                continue;
            }
            // Single-word errors are owned by vocabulary evidence: the
            // failed use already scores the lemma/form, so their new topics
            // must not also become learning items or curriculum topics.
            if open_course_core::vocabulary::vocabulary_owns_error(sentence, error) {
                continue;
            }
            for new_topic in &error.new_topics {
                if is_abstract_topic_name(&new_topic.name) {
                    log_debug_event(
                        "analysis",
                        &format!(
                            "Skipping abstract new topic from analysis: {}",
                            new_topic.name
                        ),
                        data_dir,
                    );
                    continue;
                }
                seen.entry(new_topic.name.clone())
                    .or_insert(new_topic.clone());
            }
        }
    }

    if seen.is_empty() {
        return Ok(analysis);
    }

    // Word-specific names (e.g. "Adjective: Caro vs Rico") become review cards
    // in the learning_items table, not curriculum topics. Their metadata comes
    // from the analysis itself, so no extra LLM call is needed, and their ids
    // are never added to error.topic_ids (no progress entry is created).
    let mut item_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut topic_refs: Vec<NewTopicRef> = Vec::new();
    for r in seen.into_values() {
        if open_course_core::learning_items::is_learning_item_name(&r.name) {
            let item = open_course_core::learning_items::LearningItem {
                id: open_course_core::learning_items::LearningItem::slug_id(
                    &r.name,
                    &profile.target_language,
                ),
                name: r.name,
                description: r.description,
                level: r.level,
                target_lang: profile.target_language.clone(),
                native_lang: profile.native_language.clone(),
                ..Default::default()
            };
            if item_ids.insert(item.id.clone()) {
                analysis.new_learning_items.push(item);
            }
        } else {
            topic_refs.push(r);
        }
    }

    if topic_refs.is_empty() {
        return Ok(analysis);
    }

    let generated_results = join_all(
        topic_refs
            .iter()
            .map(|r| generate_new_topic(client, profile, r, stream_tx, data_dir)),
    )
    .await;
    let mut generated_topics = Vec::new();
    for result in generated_results {
        generated_topics.push(result?);
    }

    let existing_ids: std::collections::HashSet<String> =
        existing_topics.iter().map(|t| t.id.clone()).collect();
    let mut used_ids: std::collections::HashSet<String> = existing_ids;
    for topic in &mut generated_topics {
        // Ids are derived from topic names (LLM-invented ids are not trusted).
        topic.id = open_course_core::curriculum::topic_id_from_name(&topic.name);
        if used_ids.contains(&topic.id) {
            let base = topic.id.clone();
            for i in 1.. {
                let candidate = format!("{base}-{i}");
                if !used_ids.contains(&candidate) {
                    topic.id = candidate;
                    break;
                }
            }
        }
        used_ids.insert(topic.id.clone());
    }

    let id_by_name: std::collections::HashMap<String, String> = generated_topics
        .iter()
        .map(|t| (t.name.clone(), t.id.clone()))
        .collect();

    for sentence in &mut analysis.sentences {
        for error in &mut sentence.errors {
            let mut ids = error.topic_ids.clone();
            for new_topic in &error.new_topics {
                if let Some(id) = id_by_name.get(&new_topic.name) {
                    ids.push(id.clone());
                }
            }
            error.topic_ids = unique_topic_ids(ids);
        }
    }

    analysis.new_topics.extend(generated_topics);
    Ok(analysis)
}

pub async fn generate_topic_metadata(
    client: &dyn LlmClient,
    profile: &UserProfile,
    topic_id: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Topic> {
    let prompt = build_topic_metadata_prompt(topic_id, profile);
    let response = with_timeout_secs(
        stream_or_prompt(
            client,
            &prompt,
            "Return only a valid JSON object. No markdown code fences, no explanations.",
            stream_tx,
            "Generating topic metadata...",
            None,
            MAX_TOKENS_TOPIC_METADATA,
        ),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("Topic metadata request failed for {topic_id}: {e}")))?;

    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_some() {
        log_raw_response(&prompt, &response.raw, "topic-metadata", data_dir);
    }

    let cleaned = clean_json_response(&response.raw);
    let mut topic: Topic = serde_json::from_str(&cleaned).map_err(|e| {
        AppError::Llm(format!(
            "Failed to parse topic metadata for {topic_id}: {e}; raw: {}",
            response.raw
        ))
    })?;

    if topic.id != topic_id {
        topic.id = topic_id.to_string();
    }
    normalize_topic_defaults(&mut topic, profile);

    Ok(topic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::LlmStream;
    use async_trait::async_trait;
    use open_course_core::session::{
        GrammarError, GrammarErrorType, NewTopicRef, SentenceAnalysis, VocabularyUse,
    };

    /// Any LLM call fails the test: these cases must be resolved locally.
    struct NoopClient;

    #[async_trait]
    impl LlmClient for NoopClient {
        async fn prompt(&self, _: &str, _: Option<&str>, _: u32) -> Result<String> {
            panic!("no LLM call expected");
        }

        async fn stream_prompt(&self, _: &str, _: Option<&str>, _: u32) -> Result<LlmStream> {
            panic!("no LLM call expected");
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn profile() -> UserProfile {
        UserProfile {
            native_language: "ru".to_string(),
            target_language: "es".to_string(),
            age: None,
            self_assessed_cefr: None,
        }
    }

    fn analysis_with(error: GrammarError, used_vocabulary: Vec<VocabularyUse>) -> AnalysisResult {
        AnalysisResult {
            session_score: Some(50.0),
            sentences: vec![SentenceAnalysis {
                sentence_number: 1,
                student_translation: "el pequeño casa".to_string(),
                expected_translation: "la casa pequeña".to_string(),
                acceptable_translations: vec![],
                semantic_verdict: open_course_core::session::SemanticVerdict::NeedsCorrection,
                errors: vec![error],
                per_sentence_feedback: vec![],
                used_vocabulary,
            }],
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
            new_lemmas: vec![],
            new_forms: vec![],
        }
    }

    #[tokio::test]
    async fn vocabulary_owned_error_spawns_no_learning_item() {
        // The error boils down to the single failed word "pequeño": the
        // vocabulary system owns it, so its new topic is dropped entirely.
        let error = GrammarError {
            error_type: GrammarErrorType::Major,
            pattern: "pequeño instead of pequeña".to_string(),
            explanation: "gender agreement of pequeño".to_string(),
            topic_ids: vec![],
            new_topics: vec![NewTopicRef {
                name: "Adjective: Pequeño vs Pequeña".to_string(),
                description: "gender of pequeño".to_string(),
                level: None,
            }],
        };
        let failed_use = VocabularyUse {
            surface: "pequeño".to_string(),
            lemma: "pequeño".to_string(),
            pos: "ADJ".to_string(),
            side: "student".to_string(),
            usage_ok: Some(false),
            ..Default::default()
        };
        let analysis = analysis_with(error, vec![failed_use]);

        let result = finalize_analysis_with_new_topics(
            &NoopClient,
            &profile(),
            &[],
            analysis,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(result.new_learning_items.is_empty());
        assert!(result.new_topics.is_empty());
    }

    #[tokio::test]
    async fn unowned_error_still_creates_learning_item() {
        // No failed vocabulary use overlaps the error words: the new topic
        // survives and becomes a learning item without any LLM call.
        let error = GrammarError {
            error_type: GrammarErrorType::Major,
            pattern: "caro where rico was meant".to_string(),
            explanation: "confused caro with rico".to_string(),
            topic_ids: vec![],
            new_topics: vec![NewTopicRef {
                name: "Adjective: Caro vs Rico".to_string(),
                description: "expensive vs tasty".to_string(),
                level: None,
            }],
        };
        let unrelated_use = VocabularyUse {
            surface: "casa".to_string(),
            lemma: "casa".to_string(),
            pos: "NOUN".to_string(),
            side: "student".to_string(),
            usage_ok: Some(false),
            ..Default::default()
        };
        let analysis = analysis_with(error, vec![unrelated_use]);

        let result = finalize_analysis_with_new_topics(
            &NoopClient,
            &profile(),
            &[],
            analysis,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.new_learning_items.len(), 1);
        assert_eq!(
            result.new_learning_items[0].name,
            "Adjective: Caro vs Rico"
        );
        assert!(result.new_topics.is_empty());
    }
}
