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
    target_level_topic_count,
};
use crate::error::{AppError, Result};
use crate::llm::client::LlmClient;
use crate::llm::factory::create_llm_model;
use crate::llm::prompts::{
    build_curriculum_gap_prompt, build_curriculum_level_prompt, build_new_topic_metadata_prompt,
    build_topic_metadata_prompt,
};
use crate::llm::streaming::StreamChunk;

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

    log_debug_text(
        kind,
        &format!("=== PROMPT ===\n{prompt}\n\n=== RAW RESPONSE ===\n{raw}\n"),
        data_dir,
    );
}

pub fn log_debug_event(kind: &str, message: &str, data_dir: Option<&Path>) {
    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_none() {
        return;
    }
    let Some(data_dir) = data_dir else {
        return;
    };
    log_debug_text(kind, message, data_dir);
}

fn log_debug_text(kind: &str, text: &str, data_dir: &Path) {
    let debug_dir = data_dir.join(".open-course-cli").join("debug");
    if let Err(_e) = std::fs::create_dir_all(&debug_dir) {
        return;
    }

    let file_path = debug_dir.join(format!("{kind}-{}.txt", Utc::now().timestamp_millis()));
    if std::fs::write(&file_path, text).is_err() {
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&file_path)
            .map(|m| m.permissions())
            .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o600));
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&file_path, perms);
    }

    let _ = cleanup_old_debug_files(&debug_dir, 10);
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
) -> Result<String> {
    use std::time::Duration;
    use tokio::time::{Instant, timeout};

    if let Some(tx) = stream_tx {
        send_stream_status(tx, level, &format!("{status_label} (starting...)"));
        let mut stream = match client.stream_prompt(prompt, Some(system)).await {
            Ok(stream) => stream,
            Err(e) => {
                log_debug_event(
                    "stream",
                    &format!("Streaming failed ({e}), falling back to non-streaming prompt"),
                    None,
                );
                return with_timeout(client.prompt(prompt, Some(system))).await;
            }
        };
        let mut raw = String::new();
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
                    return with_timeout(client.prompt(prompt, Some(system))).await;
                }
            }
        }
        Ok(raw)
    } else {
        with_timeout(client.prompt(prompt, Some(system))).await
    }
}

pub async fn generate_exercises(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Exercise>> {
    let system = "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let raw = with_timeout_secs(
        stream_or_prompt(client, prompt, system, stream_tx, "Generating exercises...", None),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("Exercise generation failed: {e}")))?;

    log_raw_response(prompt, &raw, "exercises", data_dir);

    let cleaned = clean_json_response(&raw);
    if let Ok(wrapper) = from_str::<Exercises>(&cleaned) {
        return Ok(wrapper.exercises);
    }
    if let Ok(vec) = from_str::<Vec<Exercise>>(&cleaned) {
        return Ok(vec);
    }
    Err(AppError::Llm(format!(
        "Failed to parse exercise response: {cleaned}"
    )))
}

pub async fn generate_analysis(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<AnalysisResult> {
    let system = "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.";
    let raw = with_timeout_secs(
        stream_or_prompt(client, prompt, system, stream_tx, "Analyzing answers...", None),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("Analysis request failed: {e}")))?;

    log_raw_response(prompt, &raw, "analysis", data_dir);

    let cleaned = clean_json_response(&raw);
    if let Ok(analysis) = from_str::<AnalysisResult>(&cleaned) {
        return Ok(analysis);
    }
    if let Ok(sentences) = from_str::<Vec<SentenceAnalysis>>(&cleaned) {
        return Ok(AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
        });
    }
    if let Ok(value) = from_str::<serde_json::Value>(&cleaned)
        && let Some(sentences_value) = value.get("sentences")
        && let Ok(sentences) =
            serde_json::from_value::<Vec<SentenceAnalysis>>(sentences_value.clone())
    {
        return Ok(AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
        });
    }
    Err(AppError::Llm(format!(
        "Failed to parse analysis response: {cleaned}"
    )))
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
    let raw = with_timeout_secs(
        stream_or_prompt(
            client,
            &prompt,
            system,
            stream_tx,
            "Generating new topic...",
            None,
        ),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("New topic generation failed: {e}")))?;

    log_raw_response(&prompt, &raw, "new-topic", data_dir);

    let cleaned = clean_json_response(&raw);
    let mut topic: Topic = serde_json::from_str(&cleaned).map_err(|e| {
        AppError::Llm(format!(
            "Failed to parse new topic response: {e}; raw: {raw}"
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
            for new_topic in &error.new_topics {
                seen.entry(new_topic.name.clone())
                    .or_insert(new_topic.clone());
            }
        }
    }

    if seen.is_empty() {
        return Ok(analysis);
    }

    let refs: Vec<NewTopicRef> = seen.into_values().collect();
    let generated_results = join_all(
        refs.iter()
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
    let raw = with_timeout_secs(
        stream_or_prompt(
            client,
            prompt,
            system,
            stream_tx,
            &format!("{level}: generating curriculum"),
            Some(level),
        ),
        LLM_CURRICULUM_TIMEOUT_SECONDS,
    )
    .await
    .map_err(|e| AppError::Llm(format!("Level {level} curriculum request failed: {e}")))?;

    log_raw_response(prompt, &raw, &format!("curriculum-{level}"), data_dir);

    let cleaned = clean_json_response(&raw);
    let level_curriculum: LevelCurriculum = match from_str::<LevelCurriculum>(&cleaned) {
        Ok(v) => v,
        Err(parse_err) => {
            let repaired = sanitize_curriculum_ids(&cleaned);
            from_str::<LevelCurriculum>(&repaired).map_err(|_| {
                AppError::Llm(format!(
                    "Failed to parse {level} curriculum response: {parse_err}. Raw: {raw}"
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
    with_timeout(client.prompt(prompt, None)).await
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
        let text = stream_or_prompt(
            client,
            current_prompt,
            TOPIC_REVIEW_SYSTEM_PROMPT,
            stream_tx,
            "Generating topic review...",
            None,
        )
        .await?;
        log_raw_response(current_prompt, &text, "topic-review", data_dir);
        Ok(text)
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
    let prompt = build_topic_metadata_prompt(topic_id, &config.profile);
    let client = create_llm_model(config)?;
    let raw = with_timeout_secs(
        stream_or_prompt(
            client.as_ref(),
            &prompt,
            "Return only a valid JSON object. No markdown code fences, no explanations.",
            stream_tx,
            "Generating topic metadata...",
            None,
        ),
        300,
    )
    .await
    .map_err(|e| AppError::Llm(format!("Topic metadata request failed for {topic_id}: {e}")))?;

    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_some() {
        log_raw_response(&prompt, &raw, "topic-metadata", data_dir);
    }

    let cleaned = clean_json_response(&raw);
    let mut topic: Topic = serde_json::from_str(&cleaned).map_err(|e| {
        AppError::Llm(format!(
            "Failed to parse topic metadata for {topic_id}: {e}; raw: {raw}"
        ))
    })?;

    if topic.id != topic_id {
        topic.id = topic_id.to_string();
    }
    if topic.target_lang.is_empty() {
        topic.target_lang = config.profile.target_language.clone();
    }
    if topic.native_lang.is_empty() {
        topic.native_lang = config.profile.native_language.clone();
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
