//! Live integration tests against real LLM providers. Every test skips
//! cleanly unless the matching env vars are set, so CI stays green without
//! keys. Keys come from the environment only — never commit them.
//!
//! ```sh
//! OC_LIVE_OPENAI_KEY=... OC_LIVE_GEMINI_KEY=... OC_LIVE_ALI_KEY=... \
//!     cargo test -p open-course-llm --test live -- --nocapture
//! ```
//!
//! Optional overrides: `OC_LIVE_OPENAI_MODEL`, `OC_LIVE_GEMINI_MODEL`,
//! `OC_LIVE_ALI_BASE_URL`, `OC_LIVE_ALI_ANTHROPIC_BASE_URL`,
//! `OC_LIVE_ALI_MODEL`, `OC_LIVE_ALI_ANTHROPIC_MODEL`.

use std::time::Duration;

use futures_util::StreamExt;
use open_course_config::provider::{ProviderConfig, ProviderId};
use open_course_core::error::AppError;
use open_course_llm::client::{LlmClient, RigClient, extract_typed};
use open_course_llm::model_listing::list_models;
use open_course_llm::streaming::StreamChunk;

const SCENARIO_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct Cap {
    capital: String,
}

fn config(
    api_key: &str,
    model: &str,
    base_url: Option<&str>,
    endpoint: Option<&str>,
) -> ProviderConfig {
    ProviderConfig::ApiKey {
        api_key: Some(api_key.to_string()),
        model: model.to_string(),
        base_url: base_url.map(str::to_string),
        endpoint: endpoint.map(str::to_string),
        reasoning_effort: None,
        enable_thinking: None,
    }
}

fn client(provider: ProviderId, cfg: &ProviderConfig) -> RigClient {
    RigClient::from_config(cfg, provider).expect("client should build")
}

/// 1. Model listing returns a non-empty list.
async fn check_list_models(provider: ProviderId, api_key: &str, base_url: Option<&str>) {
    let models = list_models(provider, Some(api_key), base_url)
        .await
        .expect("list_models should succeed");
    assert!(!models.is_empty(), "model list must not be empty");
    eprintln!(
        "  list_models: {} models, first: {}",
        models.len(),
        models[0].id
    );
}

/// 2. Non-streaming prompt returns sane text.
async fn check_prompt(client: &dyn LlmClient) {
    let text = client
        .prompt("Reply with exactly: OK", None, 256)
        .await
        .expect("prompt should succeed");
    assert!(!text.trim().is_empty(), "prompt reply must not be empty");
    eprintln!("  prompt: {:?}", &text[..text.len().min(40)]);
}

/// 3. Streaming yields incremental chunks and finishes.
async fn check_stream(client: &dyn LlmClient) {
    let mut stream = client
        .stream_prompt("Count from 1 to 10, separated by spaces.", None, 256)
        .await
        .expect("stream_prompt should open");
    let mut chunks = 0usize;
    let mut assembled = String::new();
    while let Some(item) = stream.next().await {
        match item.expect("stream chunk should not error") {
            StreamChunk::Content(text) => {
                chunks += 1;
                assembled.push_str(&text);
            }
            StreamChunk::Reasoning(_) => {}
        }
    }
    assert!(
        chunks >= 2,
        "expected multiple incremental chunks, got {chunks}"
    );
    assert!(
        !assembled.trim().is_empty(),
        "streamed text must not be empty"
    );
    eprintln!(
        "  stream: {chunks} content chunks, {:?}",
        &assembled[..assembled.len().min(40)]
    );
}

/// 4. Typed structured extraction (native output_schema, possibly
///    downgraded to the submit-tool extractor by the provider).
async fn check_extract(client: &dyn LlmClient) {
    let cap: Cap = extract_typed(
        client,
        "What is the capital of France? Answer with the structured data.",
        1024,
    )
    .await
    .expect("extract_typed should succeed");
    assert!(
        cap.capital.to_lowercase().contains("paris"),
        "expected Paris, got {:?}",
        cap.capital
    );
    eprintln!("  extract_typed: {:?}", cap.capital);
}

/// 5. A wrong API key must surface as a non-retryable failure (typed
///    classification: 401/403 -> auth, never ProviderUnavailable).
async fn check_wrong_key(provider: ProviderId, cfg: &ProviderConfig) {
    let client = client(provider, cfg);
    let err = client
        .prompt("Reply with exactly: OK", None, 256)
        .await
        .expect_err("wrong key must fail");
    assert!(
        !matches!(err, AppError::ProviderUnavailable(_)),
        "auth failure must not be classified as retryable: {err}"
    );
    eprintln!("  wrong key: {err}");
}

/// 6. Realistic generation with thinking off: the output must stay
///    well-formed and sensible, not just fast.
async fn check_realistic_generation(client: &dyn LlmClient) {
    let text = client
        .prompt(
            "Generate 3 B1-level discussion topics about travel as a JSON array of objects with id and name fields. Return only the JSON.",
            Some("You are a language tutor. Return ONLY valid JSON. No markdown, no commentary."),
            2048,
        )
        .await
        .expect("generation should succeed");
    let cleaned = open_course_llm::parse::clean_json_response(&text);
    let value: serde_json::Value =
        serde_json::from_str(&cleaned).expect("generation should parse as JSON");
    let topics = value.as_array().expect("expected a JSON array");
    assert_eq!(topics.len(), 3, "expected 3 topics: {cleaned}");
    for topic in topics {
        assert!(
            topic["id"].is_string() || topic["id"].is_number(),
            "topic missing id: {topic}"
        );
        assert!(topic["name"].is_string(), "topic missing name: {topic}");
    }
    eprintln!("  realistic generation: 3 well-formed topics");
}

/// Streaming with thinking disabled must not emit reasoning chunks.
async fn check_stream_has_no_reasoning(client: &dyn LlmClient) {
    let mut stream = client
        .stream_prompt("Count from 1 to 10, separated by spaces.", None, 256)
        .await
        .expect("stream_prompt should open");
    let mut reasoning_chunks = 0usize;
    let mut content_chunks = 0usize;
    while let Some(item) = stream.next().await {
        match item.expect("stream chunk should not error") {
            StreamChunk::Content(_) => content_chunks += 1,
            StreamChunk::Reasoning(_) => reasoning_chunks += 1,
        }
    }
    assert!(content_chunks > 0, "expected content chunks");
    assert_eq!(
        reasoning_chunks, 0,
        "thinking disabled must not emit reasoning chunks"
    );
    eprintln!("  stream: {content_chunks} content chunks, 0 reasoning chunks");
}

async fn run<Fut: std::future::Future>(name: &str, fut: Fut) -> Fut::Output {
    tokio::time::timeout(SCENARIO_TIMEOUT, fut)
        .await
        .unwrap_or_else(|_| panic!("{name} timed out after {SCENARIO_TIMEOUT:?}"))
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

fn openai_env() -> Option<(String, String)> {
    let key = std::env::var("OC_LIVE_OPENAI_KEY").ok()?;
    let model = std::env::var("OC_LIVE_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    Some((key, model))
}

#[tokio::test]
async fn openai_all_scenarios() {
    let Some((key, model)) = openai_env() else {
        eprintln!("OC_LIVE_OPENAI_KEY unset, skipping");
        return;
    };
    let cfg = config(&key, &model, None, None);
    let client = client(ProviderId::OpenAi, &cfg);
    run("list", check_list_models(ProviderId::OpenAi, &key, None)).await;
    run("prompt", check_prompt(&client)).await;
    run("stream", check_stream(&client)).await;
    run("extract", check_extract(&client)).await;
    run("generate", check_realistic_generation(&client)).await;
}

#[tokio::test]
async fn openai_wrong_key_is_non_retryable() {
    let Some((_, model)) = openai_env() else {
        eprintln!("OC_LIVE_OPENAI_KEY unset, skipping");
        return;
    };
    let cfg = config("sk-definitely-wrong", &model, None, None);
    run("wrong_key", check_wrong_key(ProviderId::OpenAi, &cfg)).await;
}

// ---------------------------------------------------------------------------
// Google Gemini
// ---------------------------------------------------------------------------

fn gemini_env() -> Option<(String, String)> {
    let key = std::env::var("OC_LIVE_GEMINI_KEY").ok()?;
    let model =
        std::env::var("OC_LIVE_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());
    Some((key, model))
}

#[tokio::test]
async fn gemini_all_scenarios() {
    let Some((key, model)) = gemini_env() else {
        eprintln!("OC_LIVE_GEMINI_KEY unset, skipping");
        return;
    };
    let cfg = config(&key, &model, None, None);
    let client = client(ProviderId::Google, &cfg);
    run("list", check_list_models(ProviderId::Google, &key, None)).await;
    run("prompt", check_prompt(&client)).await;
    run("stream", check_stream(&client)).await;
    run("extract", check_extract(&client)).await;
    run("generate", check_realistic_generation(&client)).await;
}

#[tokio::test]
async fn gemini_wrong_key_is_non_retryable() {
    let Some((_, model)) = gemini_env() else {
        eprintln!("OC_LIVE_GEMINI_KEY unset, skipping");
        return;
    };
    let cfg = config("definitely-wrong-key", &model, None, None);
    run("wrong_key", check_wrong_key(ProviderId::Google, &cfg)).await;
}

// ---------------------------------------------------------------------------
// Alibaba ModelStudio, OpenAI-compatible endpoint (Custom provider)
// ---------------------------------------------------------------------------

const ALI_DEFAULT_BASE_URL: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
const ALI_DEFAULT_ANTHROPIC_BASE_URL: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic";

fn ali_env() -> Option<(String, String, String)> {
    let key = std::env::var("OC_LIVE_ALI_KEY").ok()?;
    let base =
        std::env::var("OC_LIVE_ALI_BASE_URL").unwrap_or_else(|_| ALI_DEFAULT_BASE_URL.into());
    let model = std::env::var("OC_LIVE_ALI_MODEL").unwrap_or_else(|_| "qwen3.6-flash".to_string());
    Some((key, base, model))
}

#[tokio::test]
async fn ali_openai_compatible_all_scenarios() {
    let Some((key, base, model)) = ali_env() else {
        eprintln!("OC_LIVE_ALI_KEY unset, skipping");
        return;
    };
    let cfg = config(&key, &model, Some(&base), None);
    let client = client(ProviderId::Custom, &cfg);
    run(
        "list",
        check_list_models(ProviderId::Custom, &key, Some(&base)),
    )
    .await;
    run("prompt", check_prompt(&client)).await;
    run("stream", check_stream(&client)).await;
    run("extract", check_extract(&client)).await;
    run("generate", check_realistic_generation(&client)).await;
}

#[tokio::test]
async fn ali_openai_compatible_wrong_key_is_non_retryable() {
    let Some((_, base, model)) = ali_env() else {
        eprintln!("OC_LIVE_ALI_KEY unset, skipping");
        return;
    };
    let cfg = config("sk-definitely-wrong", &model, Some(&base), None);
    run("wrong_key", check_wrong_key(ProviderId::Custom, &cfg)).await;
}

/// Custom OpenAI-compatible providers default to `enable_thinking: false`;
/// the stream must not carry reasoning chunks.
#[tokio::test]
async fn ali_openai_compatible_stream_emits_no_reasoning() {
    let Some((key, base, model)) = ali_env() else {
        eprintln!("OC_LIVE_ALI_KEY unset, skipping");
        return;
    };
    let cfg = config(&key, &model, Some(&base), None);
    let client = client(ProviderId::Custom, &cfg);
    run("stream", check_stream_has_no_reasoning(&client)).await;
}

// ---------------------------------------------------------------------------
// Alibaba ModelStudio, Anthropic-compatible endpoint (Custom, messages)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ali_anthropic_compatible_all_scenarios() {
    let Some(key) = std::env::var("OC_LIVE_ALI_KEY").ok() else {
        eprintln!("OC_LIVE_ALI_KEY unset, skipping");
        return;
    };
    let base = std::env::var("OC_LIVE_ALI_ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| ALI_DEFAULT_ANTHROPIC_BASE_URL.into());
    let model = std::env::var("OC_LIVE_ALI_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "qwen3.6-flash".to_string());
    let cfg = config(&key, &model, Some(&base), Some("messages"));
    let client = client(ProviderId::Custom, &cfg);
    // Anthropic-compatible endpoints serve messages only; listing goes
    // through the OpenAI-compatible arm above.
    run("prompt", check_prompt(&client)).await;
    run("stream", check_stream(&client)).await;
    run("extract", check_extract(&client)).await;
    run("generate", check_realistic_generation(&client)).await;
}

#[tokio::test]
async fn ali_anthropic_compatible_wrong_key_is_non_retryable() {
    let Some(_) = std::env::var("OC_LIVE_ALI_KEY").ok() else {
        eprintln!("OC_LIVE_ALI_KEY unset, skipping");
        return;
    };
    let base = std::env::var("OC_LIVE_ALI_ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| ALI_DEFAULT_ANTHROPIC_BASE_URL.into());
    let model = std::env::var("OC_LIVE_ALI_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "qwen3.6-flash".to_string());
    let cfg = config("sk-definitely-wrong", &model, Some(&base), Some("messages"));
    run("wrong_key", check_wrong_key(ProviderId::Custom, &cfg)).await;
}
