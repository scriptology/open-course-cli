use std::path::Path;

use tokio::sync::mpsc;

use crate::app::LlmResult;
use crate::core::session::{AnalysisResult, Exercise};
use crate::error::Result;
use crate::llm::client::{LlmClient, extract_typed};
use crate::llm::debug_log::{log_debug_event, log_failed_response, log_raw_response};
use crate::llm::parse::{
    Exercises, analysis_parse_errors, build_parse_error, clean_json_response,
    exercise_parse_errors, parse_analysis, parse_exercises, validate_analysis_sentences,
};
use crate::llm::transport::{LlmResponse, stream_or_prompt, with_timeout_secs};

const MAX_TOKENS_EXERCISES: u32 = 8192;
const MAX_TOKENS_ANALYSIS: u32 = 16384;

/// Parameters for one run of the three-attempt generation loop.
pub(crate) struct RetryConfig<'a> {
    /// System prompt for the streaming attempt.
    pub system: &'a str,
    /// Stricter system prompt for the non-streaming retry.
    pub stricter_system: &'a str,
    /// Live status label shown while streaming.
    pub status_label: String,
    /// Curriculum level for per-level stream status routing, if any.
    pub level: Option<&'a str>,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    /// Kind string for debug-event log lines.
    pub log_kind: &'a str,
    /// Kind string for raw-response dumps; failure dumps append "-failed".
    pub raw_log_kind: String,
    /// Appended to attempt-failure log lines, e.g. " for B1".
    pub error_context: String,
    /// Placeholder recorded as cleaned text when structured extraction
    /// succeeds but returns unusable data.
    pub extraction_empty_message: &'a str,
    /// Kind used in the final user-facing parse error, e.g. "exercise".
    pub error_kind: String,
}

/// Three-attempt generation loop shared by exercises, analysis and
/// curriculum batch generation:
/// 1. streaming with the regular system prompt, parsed from cleaned JSON;
/// 2. non-streaming with a stricter system prompt, parsed the same way;
/// 3. structured extraction via tool-calling / response_format (only for
///    clients that support it; `extract` yields `Ok(None)` when the
///    extraction succeeded but the data was unusable).
/// On total failure the last response is dumped and a detailed error returned.
pub(crate) async fn generate_with_retries<T, P, X, XFut>(
    client: &dyn LlmClient,
    prompt: &str,
    config: RetryConfig<'_>,
    parse: P,
    extract: X,
    parse_errors: impl Fn(&str) -> String,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<T>
where
    P: Fn(&str, usize, usize) -> Result<T>,
    X: FnOnce() -> XFut,
    XFut: std::future::Future<Output = Result<Option<T>>>,
{
    let mut last_response = LlmResponse::empty();
    let mut last_cleaned = String::new();

    // Attempt 1: streaming (with live UI status).
    match with_timeout_secs(
        stream_or_prompt(
            client,
            prompt,
            config.system,
            stream_tx,
            &config.status_label,
            config.level,
            config.max_tokens,
        ),
        config.timeout_secs,
    )
    .await
    {
        Ok(response) => {
            last_response = response.clone();
            last_cleaned = clean_json_response(&response.raw);
            if let Ok(parsed) = parse(
                &last_cleaned,
                response.content_chars,
                response.reasoning_chars,
            ) {
                log_raw_response(prompt, &response.raw, &config.raw_log_kind, data_dir);
                return Ok(parsed);
            }
        }
        Err(e) => {
            log_debug_event(
                config.log_kind,
                &format!("Streaming attempt failed{}: {e}", config.error_context),
                data_dir,
            );
        }
    }

    // Attempt 2: non-streaming prompt with stricter system prompt.
    match with_timeout_secs(
        client.prompt(prompt, Some(config.stricter_system), config.max_tokens),
        config.timeout_secs,
    )
    .await
    {
        Ok(raw) => {
            last_response = LlmResponse::from_text(raw);
            last_cleaned = clean_json_response(&last_response.raw);
            if let Ok(parsed) = parse(
                &last_cleaned,
                last_response.content_chars,
                last_response.reasoning_chars,
            ) {
                log_raw_response(prompt, &last_response.raw, &config.raw_log_kind, data_dir);
                return Ok(parsed);
            }
        }
        Err(e) => {
            log_debug_event(
                config.log_kind,
                &format!("Non-streaming attempt failed{}: {e}", config.error_context),
                data_dir,
            );
        }
    }

    // Attempt 3: structured extraction (uses tool-calling / response_format where supported).
    match with_timeout_secs(extract(), config.timeout_secs).await {
        Ok(Some(value)) => {
            return Ok(value);
        }
        Ok(None) => {
            last_response = LlmResponse::empty();
            last_cleaned = config.extraction_empty_message.to_string();
        }
        Err(e) => {
            log_debug_event(
                config.log_kind,
                &format!(
                    "Structured extraction attempt failed{}: {e}",
                    config.error_context
                ),
                data_dir,
            );
        }
    }

    // All attempts failed: log a detailed failure dump and return a clear error.
    let parse_errors = parse_errors(&last_cleaned);
    let dump_path = log_failed_response(
        prompt,
        &last_response.raw,
        &last_cleaned,
        &parse_errors,
        &format!("{}-failed", config.raw_log_kind),
        data_dir,
    );
    Err(build_parse_error(
        &config.error_kind,
        &last_response,
        &last_cleaned,
        &parse_errors,
        dump_path.as_deref(),
    ))
}

pub async fn generate_exercises(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Exercise>> {
    let config = RetryConfig {
        system: "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.",
        stricter_system: "You are a language tutor. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.",
        status_label: "Generating exercises...".to_string(),
        level: None,
        max_tokens: MAX_TOKENS_EXERCISES,
        timeout_secs: 300,
        log_kind: "exercises",
        raw_log_kind: "exercises".to_string(),
        error_context: String::new(),
        extraction_empty_message: "(structured extraction returned empty exercises)",
        error_kind: "exercise".to_string(),
    };
    generate_with_retries(
        client,
        prompt,
        config,
        parse_exercises,
        || async move {
            let wrapper = extract_typed::<Exercises>(client, prompt, MAX_TOKENS_EXERCISES).await?;
            Ok(if wrapper.exercises.is_empty() {
                None
            } else {
                Some(wrapper.exercises)
            })
        },
        exercise_parse_errors,
        stream_tx,
        data_dir,
    )
    .await
}

pub async fn generate_analysis(
    client: &dyn LlmClient,
    prompt: &str,
    expected_sentence_count: usize,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<AnalysisResult> {
    let config = RetryConfig {
        system: "You are a language tutor. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.",
        stricter_system: "You are a language tutor. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.",
        status_label: "Analyzing answers...".to_string(),
        level: None,
        max_tokens: MAX_TOKENS_ANALYSIS,
        timeout_secs: 300,
        log_kind: "analysis",
        raw_log_kind: "analysis".to_string(),
        error_context: String::new(),
        extraction_empty_message: "(structured extraction returned incomplete sentences)",
        error_kind: "analysis".to_string(),
    };
    generate_with_retries(
        client,
        prompt,
        config,
        |cleaned, content_chars, reasoning_chars| {
            parse_analysis(
                cleaned,
                expected_sentence_count,
                content_chars,
                reasoning_chars,
            )
        },
        || async move {
            let analysis = extract_typed::<AnalysisResult>(client, prompt, MAX_TOKENS_ANALYSIS).await?;
            Ok(validate_analysis_sentences(analysis, expected_sentence_count, 0, 0).ok())
        },
        |cleaned| analysis_parse_errors(cleaned, expected_sentence_count),
        stream_tx,
        data_dir,
    )
    .await
}
