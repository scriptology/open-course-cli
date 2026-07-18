use std::path::Path;

use futures_util::future::join_all;
use tokio::sync::mpsc;

use crate::app::LlmResult;
use crate::config::profile::UserProfile;
use crate::core::session::{AnalysisResult, NewTopicRef, unique_topic_ids};
use crate::db::curriculum::{Topic, is_abstract_topic_name};
use crate::error::{AppError, Result};
use crate::llm::client::LlmClient;
use crate::llm::debug_log::{log_debug_event, log_raw_response};
use crate::llm::parse::clean_json_response;
use crate::llm::prompts::{build_new_topic_metadata_prompt, build_topic_metadata_prompt};
use crate::llm::transport::{stream_or_prompt, with_timeout_secs};

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
        topic.level = crate::db::curriculum::difficulty_to_cefr(&topic.difficulty);
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
            if error.error_type == crate::core::session::GrammarErrorType::Spelling {
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
        if crate::db::learning_items::is_learning_item_name(&r.name) {
            let item = crate::db::learning_items::LearningItem {
                id: crate::db::learning_items::LearningItem::slug_id(
                    &r.name,
                    &profile.target_language,
                ),
                name: r.name,
                description: r.description,
                level: r.level,
                target_lang: profile.target_language.clone(),
                native_lang: profile.native_language.clone(),
                score: 0.0,
                last_practiced: None,
                practice_count: 0,
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
