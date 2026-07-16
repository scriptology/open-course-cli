use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::join_all;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::timeout;

use serde_json::from_str;

use crate::app::LlmResult;
use crate::config::profile::UserProfile;
use crate::core::session::{
    AnalysisResult, Exercise, NewTopicRef, SentenceAnalysis, unique_topic_ids,
};
use crate::db::curriculum::{
    CEFR_LEVELS, CURRICULUM_DOMAIN_DESCRIPTIONS, Curriculum, Topic, cefr_to_difficulty,
    is_abstract_topic_name, target_level_topic_count,
};
use crate::error::{AppError, Result};
use crate::llm::client::{LlmClient, DEFAULT_MAX_TOKENS, extract_typed};
use crate::llm::factory::create_llm_model;
use crate::llm::prompts::{
    build_curriculum_gap_prompt, build_curriculum_level_prompt, build_new_topic_metadata_prompt,
    build_topic_metadata_prompt,
};
use crate::llm::streaming::StreamChunk;

const MAX_TOKENS_EXERCISES: u32 = 8192;
const MAX_TOKENS_ANALYSIS: u32 = 16384;
const MAX_TOKENS_CURRICULUM: u32 = 16384;
const MAX_TOKENS_TOPIC_REVIEW: u32 = 8192;
const MAX_TOKENS_TOPIC_METADATA: u32 = 4096;

#[derive(Debug, Clone)]
struct LlmResponse {
    raw: String,
    content_chars: usize,
    reasoning_chars: usize,
}

impl LlmResponse {
    fn empty() -> Self {
        Self {
            raw: String::new(),
            content_chars: 0,
            reasoning_chars: 0,
        }
    }

    fn from_text(text: String) -> Self {
        let chars = text.chars().count();
        Self {
            raw: text,
            content_chars: chars,
            reasoning_chars: 0,
        }
    }
}

fn send_status(label: &str, stream_tx: Option<&mpsc::Sender<LlmResult>>) {
    if let Some(tx) = stream_tx {
        let _ = tx.try_send(LlmResult::StreamChunk(label.to_string()));
    }
}

fn send_stream_status(
    tx: &mpsc::Sender<LlmResult>,
    level: Option<&str>,
    status: &str,
) {
    if let Some(level) = level {
        let _ = tx.try_send(LlmResult::CurriculumStreamChunk {
            level: level.to_string(),
            status: status.to_string(),
        });
    } else {
        let _ = tx.try_send(LlmResult::StreamChunk(status.to_string()));
    }
}

fn log_raw_response(prompt: &str, raw: &str, kind: &str, data_dir: Option<&Path>) {
    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_none() {
        return;
    }

    let Some(data_dir) = data_dir else {
        return;
    };

    let _ = log_debug_text(
        kind,
        &format!("=== PROMPT ===\n{prompt}\n\n=== RAW RESPONSE ===\n{raw}\n"),
        data_dir,
    );
}

/// Always writes a failure dump and returns the file path. Used even when
/// OPEN_COURSE_CLI_DEBUG is off so users can inspect why parsing failed.
fn log_failed_response(
    prompt: &str,
    raw: &str,
    cleaned: &str,
    parse_errors: &str,
    kind: &str,
    data_dir: Option<&Path>,
) -> Option<String> {
    let data_dir = data_dir?;
    let text = format!(
        "=== PROMPT ===\n{prompt}\n\n=== RAW RESPONSE ({raw_len} chars) ===\n{raw}\n\n=== CLEANED JSON ===\n{cleaned}\n\n=== PARSE ERRORS ===\n{parse_errors}\n",
        raw_len = raw.len(),
    );
    log_debug_text(kind, &text, data_dir)
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

pub fn log_debug_event(kind: &str, message: &str, data_dir: Option<&Path>) {
    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_none() {
        return;
    }
    let Some(data_dir) = data_dir else {
        return;
    };
    let _ = log_debug_text(kind, message, data_dir);
}

fn log_debug_text(kind: &str, text: &str, data_dir: &Path) -> Result<std::path::PathBuf> {
    let debug_dir = data_dir.join(".open-course-cli").join("debug");
    std::fs::create_dir_all(&debug_dir)?;

    let file_path = debug_dir.join(format!("{kind}-{}.txt", Utc::now().timestamp_millis()));
    std::fs::write(&file_path, text)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&file_path)
            .map(|m| m.permissions())
            .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o600));
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&file_path, perms);
    }

    let _ = cleanup_old_debug_files(&debug_dir, 20);
    Ok(file_path)
}

fn cleanup_old_debug_files(debug_dir: &Path, keep: usize) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(debug_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .collect();
    if entries.len() <= keep {
        return Ok(());
    }
    entries.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    entries.reverse();
    for entry in entries.iter().skip(keep) {
        let _ = std::fs::remove_file(entry.path());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Exercises {
    exercises: Vec<Exercise>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct LevelCurriculum {
    topics: Vec<Topic>,
}

const LLM_TIMEOUT_SECONDS: u64 = 60;
const LLM_CURRICULUM_TIMEOUT_SECONDS: u64 = 300;

async fn with_timeout<T, F>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    with_timeout_secs(future, LLM_TIMEOUT_SECONDS).await
}

async fn with_timeout_secs<T, F>(future: F, secs: u64) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    timeout(Duration::from_secs(secs), future)
        .await
        .map_err(|_| AppError::Llm(format!("LLM request timed out after {secs}s")))?
}

async fn stream_or_prompt(
    client: &dyn LlmClient,
    prompt: &str,
    system: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    status_label: &str,
    level: Option<&str>,
    max_tokens: u32,
) -> Result<LlmResponse> {
    use std::time::Duration;
    use tokio::time::{Instant, timeout};

    if let Some(tx) = stream_tx {
        send_stream_status(tx, level, &format!("{status_label} (starting...)"));
        let mut stream = match client.stream_prompt(prompt, Some(system), max_tokens).await {
            Ok(stream) => stream,
            Err(e) => {
                log_debug_event(
                    "stream",
                    &format!("Streaming failed ({e}), falling back to non-streaming prompt"),
                    None,
                );
                let text = with_timeout_secs(
                    client.prompt(prompt, Some(system), max_tokens),
                    LLM_TIMEOUT_SECONDS,
                )
                .await?;
                return Ok(LlmResponse::from_text(text));
            }
        };
        let mut raw = String::new();
        let mut reasoning_text = String::new();
        let mut content_chars = 0usize;
        let mut reasoning_chars = 0usize;
        let idle_timeout = Duration::from_secs(45);
        let overall_timeout = Duration::from_secs(300);
        let start = Instant::now();

        loop {
            if start.elapsed() > overall_timeout {
                return Err(AppError::Llm(format!(
                    "{status_label} stream exceeded overall timeout of 300s"
                )));
            }
            let chunk = match timeout(idle_timeout, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    return Err(AppError::Llm(format!(
                        "{status_label} stream idle timeout after 45s"
                    )));
                }
            };
            match chunk {
                Ok(StreamChunk::Content(text)) => {
                    content_chars += text.chars().count();
                    raw.push_str(&text);
                    let status = format!("{status_label} (writing {content_chars} chars)");
                    send_stream_status(tx, level, &status);
                }
                Ok(StreamChunk::Reasoning(text)) => {
                    reasoning_chars += text.chars().count();
                    reasoning_text.push_str(&text);
                    let status = format!("{status_label} (thinking {reasoning_chars} chars)");
                    send_stream_status(tx, level, &status);
                }
                Err(e) => {
                    log_debug_event(
                        "stream",
                        &format!(
                            "Streaming chunk failed ({e}), falling back to non-streaming prompt"
                        ),
                        None,
                    );
                    let text = with_timeout_secs(
                        client.prompt(prompt, Some(system), max_tokens),
                        LLM_TIMEOUT_SECONDS,
                    )
                    .await?;
                    return Ok(LlmResponse::from_text(text));
                }
            }
        }
        if raw.trim().is_empty() && !reasoning_text.trim().is_empty() {
            log_debug_event(
                "stream",
                &format!(
                    "{status_label} stream returned no content chunks; using reasoning text as fallback ({reasoning_chars} chars)"
                ),
                None,
            );
            return Ok(LlmResponse {
                raw: reasoning_text,
                content_chars: reasoning_chars,
                reasoning_chars,
            });
        }
        Ok(LlmResponse {
            raw,
            content_chars,
            reasoning_chars,
        })
    } else {
        let text = with_timeout_secs(
            client.prompt(prompt, Some(system), max_tokens),
            LLM_TIMEOUT_SECONDS,
        )
        .await?;
        Ok(LlmResponse::from_text(text))
    }
}

pub async fn generate_exercises(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Exercise>> {
    let system = "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let stricter_system = "You are a language tutor. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.";

    let mut last_response = LlmResponse::empty();
    let mut last_cleaned = String::new();

    // Attempt 1: streaming (with live UI status).
    match with_timeout_secs(
        stream_or_prompt(
            client,
            prompt,
            system,
            stream_tx,
            "Generating exercises...",
            None,
            MAX_TOKENS_EXERCISES,
        ),
        300,
    )
    .await
    {
        Ok(response) => {
            last_response = response.clone();
            last_cleaned = clean_json_response(&response.raw);
            if let Ok(exercises) = parse_exercises(
                &last_cleaned,
                response.content_chars,
                response.reasoning_chars,
            ) {
                log_raw_response(prompt, &response.raw, "exercises", data_dir);
                return Ok(exercises);
            }
        }
        Err(e) => {
            log_debug_event("exercises", &format!("Streaming attempt failed: {e}"), data_dir);
        }
    }

    // Attempt 2: non-streaming prompt with stricter system prompt.
    match with_timeout_secs(
        client.prompt(prompt, Some(stricter_system), MAX_TOKENS_EXERCISES),
        300,
    )
    .await
    {
        Ok(raw) => {
            last_response = LlmResponse::from_text(raw);
            last_cleaned = clean_json_response(&last_response.raw);
            if let Ok(exercises) = parse_exercises(
                &last_cleaned,
                last_response.content_chars,
                last_response.reasoning_chars,
            ) {
                log_raw_response(prompt, &last_response.raw, "exercises", data_dir);
                return Ok(exercises);
            }
        }
        Err(e) => {
            log_debug_event("exercises", &format!("Non-streaming attempt failed: {e}"), data_dir);
        }
    }

    // Attempt 3: structured extraction (uses tool-calling / response_format where supported).
    match with_timeout_secs(
        extract_typed::<Exercises>(client, prompt, MAX_TOKENS_EXERCISES),
        300,
    )
    .await
    {
        Ok(wrapper) => {
            if !wrapper.exercises.is_empty() {
                return Ok(wrapper.exercises);
            }
            last_response = LlmResponse::empty();
            last_cleaned = "(structured extraction returned empty exercises)".to_string();
        }
        Err(e) => {
            log_debug_event(
                "exercises",
                &format!("Structured extraction attempt failed: {e}"),
                data_dir,
            );
        }
    }

    // All attempts failed: log a detailed failure dump and return a clear error.
    let parse_errors = exercise_parse_errors(&last_cleaned);
    let dump_path = log_failed_response(
        prompt,
        &last_response.raw,
        &last_cleaned,
        &parse_errors,
        "exercises-failed",
        data_dir,
    );
    Err(build_parse_error(
        "exercise",
        &last_response,
        &last_cleaned,
        &parse_errors,
        dump_path.as_deref(),
    ))
}

fn parse_exercises(
    cleaned: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Vec<Exercise>> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    if let Ok(wrapper) = from_str::<Exercises>(cleaned) {
        if wrapper.exercises.is_empty() {
            return Err(AppError::Llm("parsed JSON contains no exercises".to_string()));
        }
        return Ok(wrapper.exercises);
    }
    if let Ok(vec) = from_str::<Vec<Exercise>>(cleaned) {
        if vec.is_empty() {
            return Err(AppError::Llm("parsed JSON array is empty".to_string()));
        }
        return Ok(vec);
    }

    Err(AppError::Llm("JSON does not match expected exercise schema".to_string()))
}

fn exercise_parse_errors(cleaned: &str) -> String {
    let wrapper_err = from_str::<Exercises>(cleaned)
        .err()
        .map(|e| format!("as {{exercises}}: {e}"))
        .unwrap_or_default();
    let vec_err = from_str::<Vec<Exercise>>(cleaned)
        .err()
        .map(|e| format!("as array: {e}"))
        .unwrap_or_default();
    format!("{wrapper_err}; {vec_err}")
}

fn build_parse_error(
    kind: &str,
    response: &LlmResponse,
    cleaned: &str,
    parse_errors: &str,
    dump_path: Option<&str>,
) -> AppError {
    let raw_preview = if response.raw.len() > 500 {
        format!(
            "{}...[truncated, total {} chars]",
            &response.raw[..500],
            response.raw.len()
        )
    } else {
        response.raw.clone()
    };
    let cleaned_preview = if cleaned.len() > 500 {
        format!("{}...[truncated, total {} chars]", &cleaned[..500], cleaned.len())
    } else {
        cleaned.to_string()
    };
    let dump_hint = dump_path
        .map(|p| format!("\nFull dump written to: {p}"))
        .unwrap_or_default();
    AppError::Llm(format!(
        "Failed to generate {kind} after all retries.\nRaw ({raw_len} chars, content {content} chars, reasoning {reasoning} chars): {raw_preview}\nCleaned: {cleaned_preview}\nParse errors: {parse_errors}{dump_hint}",
        raw_len = response.raw.len(),
        content = response.content_chars,
        reasoning = response.reasoning_chars,
    ))
}

pub async fn generate_analysis(
    client: &dyn LlmClient,
    prompt: &str,
    expected_sentence_count: usize,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<AnalysisResult> {
    let system = "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let stricter_system = "You are a language tutor. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.";

    let mut last_response = LlmResponse::empty();
    let mut last_cleaned = String::new();

    // Attempt 1: streaming.
    match with_timeout_secs(
        stream_or_prompt(
            client,
            prompt,
            system,
            stream_tx,
            "Analyzing answers...",
            None,
            MAX_TOKENS_ANALYSIS,
        ),
        300,
    )
    .await
    {
        Ok(response) => {
            last_response = response.clone();
            last_cleaned = clean_json_response(&response.raw);
            if let Ok(analysis) = parse_analysis(
                &last_cleaned,
                expected_sentence_count,
                response.content_chars,
                response.reasoning_chars,
            ) {
                log_raw_response(prompt, &response.raw, "analysis", data_dir);
                return Ok(analysis);
            }
        }
        Err(e) => {
            log_debug_event("analysis", &format!("Streaming attempt failed: {e}"), data_dir);
        }
    }

    // Attempt 2: non-streaming prompt with stricter system prompt.
    match with_timeout_secs(
        client.prompt(prompt, Some(stricter_system), MAX_TOKENS_ANALYSIS),
        300,
    )
    .await
    {
        Ok(raw) => {
            last_response = LlmResponse::from_text(raw);
            last_cleaned = clean_json_response(&last_response.raw);
            if let Ok(analysis) = parse_analysis(
                &last_cleaned,
                expected_sentence_count,
                last_response.content_chars,
                last_response.reasoning_chars,
            ) {
                log_raw_response(prompt, &last_response.raw, "analysis", data_dir);
                return Ok(analysis);
            }
        }
        Err(e) => {
            log_debug_event("analysis", &format!("Non-streaming attempt failed: {e}"), data_dir);
        }
    }

    // Attempt 3: structured extraction.
    match with_timeout_secs(
        extract_typed::<AnalysisResult>(client, prompt, MAX_TOKENS_ANALYSIS),
        300,
    )
    .await
    {
        Ok(analysis) => {
            if let Ok(validated) = validate_analysis_sentences(
                analysis,
                expected_sentence_count,
                0,
                0,
            ) {
                return Ok(validated);
            }
            last_response = LlmResponse::empty();
            last_cleaned = "(structured extraction returned incomplete sentences)".to_string();
        }
        Err(e) => {
            log_debug_event(
                "analysis",
                &format!("Structured extraction attempt failed: {e}"),
                data_dir,
            );
        }
    }

    let parse_errors = analysis_parse_errors(&last_cleaned, expected_sentence_count);
    let dump_path = log_failed_response(
        prompt,
        &last_response.raw,
        &last_cleaned,
        &parse_errors,
        "analysis-failed",
        data_dir,
    );
    Err(build_parse_error(
        "analysis",
        &last_response,
        &last_cleaned,
        &parse_errors,
        dump_path.as_deref(),
    ))
}

fn parse_analysis(
    cleaned: &str,
    expected_sentence_count: usize,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<AnalysisResult> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    let analysis: AnalysisResult = if let Ok(analysis) = from_str::<AnalysisResult>(cleaned) {
        analysis
    } else if let Ok(sentences) = from_str::<Vec<SentenceAnalysis>>(cleaned) {
        AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
        }
    } else if let Ok(value) = from_str::<serde_json::Value>(cleaned)
        && let Some(sentences_value) = value.get("sentences")
        && let Ok(sentences) =
            serde_json::from_value::<Vec<SentenceAnalysis>>(sentences_value.clone())
    {
        AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
        }
    } else {
        return Err(AppError::Llm(
            "JSON does not match expected analysis schema".to_string(),
        ));
    };

    validate_analysis_sentences(analysis, expected_sentence_count, content_chars, reasoning_chars)
}

fn validate_analysis_sentences(
    mut analysis: AnalysisResult,
    expected_sentence_count: usize,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<AnalysisResult> {
    if analysis.sentences.is_empty() {
        return Err(AppError::Llm(format!(
            "analysis has no sentences (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }
    if analysis.sentences.len() != expected_sentence_count {
        return Err(AppError::Llm(format!(
            "expected {expected_sentence_count} sentences, got {actual}",
            actual = analysis.sentences.len()
        )));
    }
    // Fill missing sentence numbers if the model skipped them.
    for (i, sentence) in analysis.sentences.iter_mut().enumerate() {
        if sentence.sentence_number <= 0 {
            sentence.sentence_number = (i + 1) as i32;
        }
    }
    Ok(analysis)
}

fn analysis_parse_errors(cleaned: &str, expected_sentence_count: usize) -> String {
    let top_err = from_str::<AnalysisResult>(cleaned)
        .err()
        .map(|e| format!("top-level: {e}"))
        .unwrap_or_default();
    let arr_err = from_str::<Vec<SentenceAnalysis>>(cleaned)
        .err()
        .map(|e| format!("as array: {e}"))
        .unwrap_or_default();
    format!(
        "expected {expected_sentence_count} sentences; top-level: {top_err}; array: {arr_err}"
    )
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

pub async fn generate_curriculum(
    client: &dyn LlmClient,
    profile: &UserProfile,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Curriculum> {
    let start_index = current_cefr_index(profile.self_assessed_cefr.as_deref());
    let levels: Vec<&str> = CEFR_LEVELS.iter().skip(start_index).copied().collect();

    send_status("Generating curriculum plan...", stream_tx);

    let level_futures: Vec<_> = levels
        .iter()
        .map(|&level| {
            let level_owned = level.to_string();
            async move {
                let previous = previous_cefr_level(&level_owned);
                let count = target_level_topic_count(&level_owned);
                let prompt = build_curriculum_level_prompt(
                    profile,
                    &level_owned,
                    previous.as_deref(),
                    count,
                );
                let mut topics = generate_curriculum_batch(
                    client,
                    &prompt,
                    &level_owned,
                    stream_tx,
                    data_dir,
                )
                .await?;
                normalize_level_topics(&mut topics, profile, &level_owned);
                topics = fill_level_gaps(
                    client,
                    profile,
                    &level_owned,
                    topics,
                    stream_tx,
                    data_dir,
                )
                .await?;
                normalize_level_topics(&mut topics, profile, &level_owned);
                if let Some(tx) = stream_tx {
                    let _ = tx
                        .send(LlmResult::CurriculumStreamChunk {
                            level: level_owned.clone(),
                            status: format!("{level_owned} complete"),
                        })
                        .await;
                }
                Ok::<Vec<Topic>, AppError>(topics)
            }
        })
        .collect();

    let mut all_topics = Vec::new();
    for result in join_all(level_futures).await {
        all_topics.extend(result?);
    }

    send_status("Curriculum generation complete", stream_tx);

    Ok(Curriculum {
        version: 1,
        target_language: profile.target_language.clone(),
        native_language: profile.native_language.clone(),
        topics: all_topics,
    })
}

fn previous_cefr_level(level: &str) -> Option<String> {
    let idx = CEFR_LEVELS
        .iter()
        .position(|&l| l == level.to_uppercase())?;
    idx.checked_sub(1).map(|i| CEFR_LEVELS[i].to_string())
}

fn current_cefr_index(cefr: Option<&str>) -> usize {
    let cefr = cefr
        .map(|c| c.to_uppercase())
        .unwrap_or_else(|| "A1".to_string());
    CEFR_LEVELS.iter().position(|&l| l == cefr).unwrap_or(0)
}

fn normalize_level_topics(topics: &mut [Topic], profile: &UserProfile, level: &str) {
    for topic in topics.iter_mut() {
        topic.id = topic
            .id
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if topic.id.is_empty() {
            topic.id = "topic".to_string();
        }
        if topic.target_lang.is_empty() {
            topic.target_lang = profile.target_language.clone();
        }
        if topic.native_lang.is_empty() {
            topic.native_lang = profile.native_language.clone();
        }
        if topic.level.is_none() {
            topic.level = Some(level.to_string());
        }
        if topic.difficulty.is_empty() {
            topic.difficulty = cefr_to_difficulty(level).to_string();
        }
    }
}

async fn generate_curriculum_batch(
    client: &dyn LlmClient,
    prompt: &str,
    level: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Topic>> {
    let system = "You are a curriculum design assistant. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let stricter_system = "You are a curriculum design assistant. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.";

    let mut last_response = LlmResponse::empty();
    let mut last_cleaned = String::new();

    // Attempt 1: streaming.
    match with_timeout_secs(
        stream_or_prompt(
            client,
            prompt,
            system,
            stream_tx,
            &format!("{level}: generating curriculum"),
            Some(level),
            MAX_TOKENS_CURRICULUM,
        ),
        LLM_CURRICULUM_TIMEOUT_SECONDS,
    )
    .await
    {
        Ok(response) => {
            last_response = response.clone();
            last_cleaned = clean_json_response(&response.raw);
            if let Ok(topics) = parse_curriculum_level(
                &last_cleaned,
                level,
                response.content_chars,
                response.reasoning_chars,
            ) {
                log_raw_response(prompt, &response.raw, &format!("curriculum-{level}"), data_dir);
                return Ok(topics);
            }
        }
        Err(e) => {
            log_debug_event(
                "curriculum",
                &format!("Streaming attempt failed for {level}: {e}"),
                data_dir,
            );
        }
    }

    // Attempt 2: non-streaming prompt.
    match with_timeout_secs(
        client.prompt(prompt, Some(stricter_system), MAX_TOKENS_CURRICULUM),
        LLM_CURRICULUM_TIMEOUT_SECONDS,
    )
    .await
    {
        Ok(raw) => {
            last_response = LlmResponse::from_text(raw);
            last_cleaned = clean_json_response(&last_response.raw);
            if let Ok(topics) = parse_curriculum_level(
                &last_cleaned,
                level,
                last_response.content_chars,
                last_response.reasoning_chars,
            ) {
                log_raw_response(prompt, &last_response.raw, &format!("curriculum-{level}"), data_dir);
                return Ok(topics);
            }
        }
        Err(e) => {
            log_debug_event(
                "curriculum",
                &format!("Non-streaming attempt failed for {level}: {e}"),
                data_dir,
            );
        }
    }

    // Attempt 3: structured extraction.
    match with_timeout_secs(
        extract_typed::<LevelCurriculum>(client, prompt, MAX_TOKENS_CURRICULUM),
        LLM_CURRICULUM_TIMEOUT_SECONDS,
    )
    .await
    {
        Ok(level_curriculum) => {
            if !level_curriculum.topics.is_empty() {
                return Ok(level_curriculum.topics);
            }
            last_response = LlmResponse::empty();
            last_cleaned = "(structured extraction returned empty topics)".to_string();
        }
        Err(e) => {
            log_debug_event(
                "curriculum",
                &format!("Structured extraction attempt failed for {level}: {e}"),
                data_dir,
            );
        }
    }

    let parse_errors = curriculum_parse_errors(&last_cleaned, level);
    let dump_path = log_failed_response(
        prompt,
        &last_response.raw,
        &last_cleaned,
        &parse_errors,
        &format!("curriculum-{level}-failed"),
        data_dir,
    );
    Err(build_parse_error(
        &format!("{level} curriculum"),
        &last_response,
        &last_cleaned,
        &parse_errors,
        dump_path.as_deref(),
    ))
}

fn parse_curriculum_level(
    cleaned: &str,
    level: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Vec<Topic>> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    let level_curriculum: LevelCurriculum = match from_str::<LevelCurriculum>(cleaned) {
        Ok(v) => v,
        Err(parse_err) => {
            let repaired = sanitize_curriculum_ids(cleaned);
            from_str::<LevelCurriculum>(&repaired).map_err(|_| {
                AppError::Llm(format!(
                    "Failed to parse {level} curriculum response: {parse_err}"
                ))
            })?
        }
    };

    if level_curriculum.topics.is_empty() {
        return Err(AppError::Llm(format!(
            "Level {level} curriculum returned no topics"
        )));
    }

    Ok(level_curriculum.topics)
}

fn curriculum_parse_errors(cleaned: &str, level: &str) -> String {
    from_str::<LevelCurriculum>(cleaned)
        .err()
        .map(|e| format!("{level} curriculum parse: {e}"))
        .unwrap_or_default()
}

fn clean_json_response(raw: &str) -> String {
    // Replace raw newlines with spaces so models that emit multi-line strings
    // without escaping do not break JSON parsing.
    let trimmed = raw.trim().replace('\r', "").replace('\n', " ");
    let start = [trimmed.find('{'), trimmed.find('[')]
        .into_iter()
        .flatten()
        .min();
    if let Some(start) = start {
        let bytes = trimmed.as_bytes();
        let open = bytes[start];
        let close = if open == b'{' { b'}' } else { b']' };
        let mut depth = 1;
        let mut in_string = false;
        let mut escape = false;
        for i in (start + 1)..bytes.len() {
            let c = bytes[i];
            if in_string {
                if escape {
                    escape = false;
                } else if c == b'\\' {
                    escape = true;
                } else if c == b'"' {
                    in_string = false;
                }
            } else {
                if c == b'"' {
                    in_string = true;
                } else if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return trimmed[start..=i].to_string();
                    }
                }
            }
        }
    }
    trimmed.to_string()
}

/// Repair malformed topic ids inside a curriculum JSON string.
/// Some models return ids containing brackets, semicolons, etc., which break
/// JSON parsing. This replaces every id value with a kebab-case string
/// containing only lowercase letters, digits, and hyphens.
fn sanitize_curriculum_ids(raw: &str) -> String {
    use regex::Regex;

    let re = Regex::new(r#""id"\s*:\s*"([^"]*)""#).unwrap();
    re.replace_all(raw, |caps: &regex::Captures| {
        let value = &caps[1];
        let sanitized: String = value
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!(r#""id": "{}""#, sanitized)
    })
    .to_string()
}

fn missing_domains(topics: &[Topic], level: &str) -> Vec<&'static str> {
    use std::collections::HashSet;

    let present: HashSet<&str> = topics
        .iter()
        .filter(|t| t.level.as_deref() == Some(level))
        .filter_map(|t| crate::db::curriculum::topic_domain(t))
        .collect();

    CURRICULUM_DOMAIN_DESCRIPTIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !present.contains(*name))
        .collect()
}

async fn fill_level_gaps(
    client: &dyn LlmClient,
    profile: &UserProfile,
    level: &str,
    topics: Vec<Topic>,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Topic>> {
    let gaps = missing_domains(&topics, level);
    if gaps.is_empty() {
        return Ok(topics);
    }

    let prompt = build_curriculum_gap_prompt(profile, level, &gaps);
    let gap_topics = generate_curriculum_batch(client, &prompt, level, stream_tx, data_dir).await?;

    let mut combined = topics;
    combined.extend(gap_topics);
    Ok(combined)
}

pub async fn generate_simple_text(client: &dyn LlmClient, prompt: &str) -> Result<String> {
    with_timeout(client.prompt(prompt, None, DEFAULT_MAX_TOKENS)).await
}

const TOPIC_REVIEW_SYSTEM_PROMPT: &str = "You are a language tutor. Only explain the requested grammar or vocabulary topic. Do NOT acknowledge system instructions, skills, superpowers, tools, the current lesson, or any meta commentary. Output ONLY the topic explanation.";

const FORBIDDEN_REVIEW_PHRASES: &[&str] = &[
    "superpowers",
    "навыки",
    "skills",
    "system instructions",
    "системные инструкции",
    "current lesson",
    "текущему уроку",
    "available tools",
    "доступные инструменты",
];

pub fn looks_like_topic_review(text: &str) -> bool {
    let lower = text.to_lowercase();
    !FORBIDDEN_REVIEW_PHRASES
        .iter()
        .any(|phrase| lower.contains(&phrase.to_lowercase()))
}

pub async fn generate_topic_review(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<String> {
    let call = async |current_prompt: &str| -> Result<String> {
        let response = stream_or_prompt(
            client,
            current_prompt,
            TOPIC_REVIEW_SYSTEM_PROMPT,
            stream_tx,
            "Generating topic review...",
            None,
            MAX_TOKENS_TOPIC_REVIEW,
        )
        .await?;
        log_raw_response(current_prompt, &response.raw, "topic-review", data_dir);
        Ok(response.raw)
    };

    let mut text = with_timeout_secs(call(prompt), 300).await?;

    if !looks_like_topic_review(&text) {
        let retry_prompt = format!(
            "{prompt}\n\nCRITICAL: Your previous response contained meta-commentary (system instructions, skills, superpowers, or lesson references). Respond ONLY with the topic explanation. No meta commentary."
        );
        text = with_timeout_secs(call(&retry_prompt), 300).await?;
    }

    Ok(text)
}

pub async fn generate_topic_metadata(
    config: &crate::config::OpenCourseConfig,
    topic_id: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Topic> {
    let prompt = build_topic_metadata_prompt(topic_id, config.active_profile());
    let client = create_llm_model(config)?;
    let response = with_timeout_secs(
        stream_or_prompt(
            client.as_ref(),
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
    if topic.target_lang.is_empty() {
        topic.target_lang = config.active_profile().target_language.clone();
    }
    if topic.native_lang.is_empty() {
        topic.native_lang = config.active_profile().native_language.clone();
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

    Ok(topic)
}
