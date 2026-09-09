use std::any::Any;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use rig::agent::Agent;
use rig::client::{AgentClientExt, AgentModelExt, CompletionClient};
use rig::completion::{
    CompletionError, CompletionModel, Prompt, PromptError, StructuredOutputError, TypedPrompt,
};
use rig::extractor::{ExtractionError, ExtractorBuilder};
use rig::providers::{anthropic, gemini, openai};

use crate::provider::ProviderMeta;
use crate::streaming::LlmStream;
use open_course_config::provider::{ProviderConfig, ProviderId};
use open_course_core::error::{AppError, Result};

const LLM_MAX_RETRIES: usize = 3;

/// Ceiling applied to a provider-supplied `Retry-After` hint so a hostile
/// or buggy value cannot park a retry loop for minutes.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

pub const DEFAULT_MAX_TOKENS: u32 = 8192;

fn is_provider_unavailable(msg: &str) -> bool {
    msg.contains("Inference is temporarily unavailable")
        || msg.contains("failover_exhausted")
        || msg.contains("temporarily unavailable")
        || msg.contains("server_error")
}

fn provider_error_message(msg: &str) -> String {
    if let Some(start) = msg.find("{\"error\"") {
        let json_str = &msg[start..];
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str)
            && let Some(message) = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
        {
            return message.to_string();
        }
    }
    msg.to_string()
}

/// Typed provider-error inspection backing [`classify_llm_error`]. rig's
/// error types expose the HTTP status, response body, provider request id
/// and rate-limit headers of the failed call; errors that carry no typed
/// data (plain strings, our own messages) get the default `None`s and are
/// classified by string-matching.
pub(crate) trait TypedErrorInfo: std::fmt::Display {
    fn status(&self) -> Option<u16> {
        None
    }
    fn body(&self) -> Option<&str> {
        None
    }
    fn request_id(&self) -> Option<&str> {
        None
    }
    fn retry_after(&self) -> Option<Duration> {
        None
    }
}

macro_rules! impl_typed_error_info {
    ($ty:ty) => {
        impl TypedErrorInfo for $ty {
            fn status(&self) -> Option<u16> {
                self.provider_response_status().map(|s| s.as_u16())
            }
            fn body(&self) -> Option<&str> {
                self.provider_response_body()
            }
            fn request_id(&self) -> Option<&str> {
                self.provider_request_id()
            }
            fn retry_after(&self) -> Option<Duration> {
                // Only the delta-seconds form is useful; the HTTP-date form
                // is ignored.
                self.provider_response_headers()?
                    .get("retry-after")?
                    .to_str()
                    .ok()?
                    .parse::<u64>()
                    .ok()
                    .map(Duration::from_secs)
            }
        }
    };
}

impl_typed_error_info!(CompletionError);
impl_typed_error_info!(PromptError);
impl_typed_error_info!(StructuredOutputError);

impl TypedErrorInfo for ExtractionError {
    fn status(&self) -> Option<u16> {
        match self {
            Self::CompletionError(e) => e.status(),
            Self::PromptError(e) => e.status(),
            _ => None,
        }
    }
    fn body(&self) -> Option<&str> {
        match self {
            Self::CompletionError(e) => e.body(),
            Self::PromptError(e) => e.body(),
            _ => None,
        }
    }
    fn request_id(&self) -> Option<&str> {
        match self {
            Self::CompletionError(e) => e.request_id(),
            Self::PromptError(e) => e.request_id(),
            _ => None,
        }
    }
    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::CompletionError(e) => e.retry_after(),
            Self::PromptError(e) => e.retry_after(),
            _ => None,
        }
    }
}

impl TypedErrorInfo for &str {}
impl TypedErrorInfo for String {}

/// Best human-readable message for an LLM failure: the provider's own
/// `error.message` from the typed response body when there is one, else the
/// legacy scrape of the Display output (which falls back to the full
/// Display string).
fn error_message<E: TypedErrorInfo>(e: &E) -> String {
    if let Some(message) = e
        .body()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
    {
        return message;
    }
    provider_error_message(&e.to_string())
}

/// Classify an LLM failure into the crate error type, typed data first:
/// 429/5xx are transient (`ProviderUnavailable`, drives the retry loops
/// and the streaming→prompt fallback), auth failures and other statuses
/// are not retried. With no typed status (connect errors, rig-generated
/// diagnostics, our own strings) the substring heuristic decides. The
/// provider request id is appended when the error carries one.
pub(crate) fn classify_llm_error<E: TypedErrorInfo>(e: E) -> AppError {
    let message = match e.request_id() {
        Some(id) => format!("{} (provider request id: {id})", error_message(&e)),
        None => error_message(&e),
    };
    match e.status() {
        Some(429) | Some(500..=599) => AppError::ProviderUnavailable(message),
        Some(status @ (401 | 403)) => AppError::Llm(format!(
            "authentication failed (status {status}): {message}"
        )),
        Some(_) => AppError::Llm(message),
        None if is_provider_unavailable(&e.to_string()) => AppError::ProviderUnavailable(message),
        None => AppError::Llm(message),
    }
}

/// Backoff before the next retry: the provider's `Retry-After` hint when
/// one was captured (capped), else the fixed linear backoff.
pub(crate) fn retry_delay(attempt: usize, retry_after: Option<Duration>) -> Duration {
    retry_after
        .map(|delay| delay.min(MAX_RETRY_AFTER))
        .unwrap_or_else(|| Duration::from_millis(500 * attempt as u64))
}

/// `additional_params` that keep reasoning off on Anthropic-family APIs:
/// newer Claude models (e.g. claude-sonnet-5) enable adaptive thinking by
/// default, which adds latency and burns output tokens, and MiniMax-M3 is
/// guarded against a thinking-on default change (older models accept but
/// ignore this field).
fn anthropic_disable_thinking_params() -> serde_json::Value {
    serde_json::json!({ "thinking": { "type": "disabled" } })
}

#[async_trait]
pub trait LlmClient: Send + Sync + Any {
    async fn prompt(&self, prompt: &str, system: Option<&str>, max_tokens: u32) -> Result<String>;
    async fn stream_prompt(
        &self,
        prompt: &str,
        system: Option<&str>,
        max_tokens: u32,
    ) -> Result<LlmStream>;

    fn as_any(&self) -> &dyn Any;
}

/// Helper to run a typed structured-extraction call on any `LlmClient`.
/// Works by downcasting to the known concrete implementations.
pub async fn extract_typed<T: DeserializeOwned + JsonSchema + Send + Sync + Serialize + 'static>(
    client: &dyn LlmClient,
    prompt: &str,
    max_tokens: u32,
) -> Result<T> {
    let rig = as_rig_client(client).ok_or_else(|| {
        AppError::Llm("Unsupported LLM client implementation for structured extraction".to_string())
    })?;
    rig.extract_typed_impl::<T>(prompt, max_tokens).await
}

fn as_rig_client(client: &dyn LlmClient) -> Option<&RigClient> {
    if let Some(rig) = client.as_any().downcast_ref::<RigClient>() {
        return Some(rig);
    }
    if let Some(diag) = client
        .as_any()
        .downcast_ref::<crate::diagnostics::DiagnosticLlmClient>()
    {
        return as_rig_client(diag.inner());
    }
    None
}

enum RigClientInner {
    OpenAi(openai::CompletionsClient),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
}

pub struct RigClient {
    inner: RigClientInner,
    model: String,
    reasoning_effort: Option<String>,
    enable_thinking: Option<bool>,
    /// Anthropic-family providers (Anthropic, MiniMax, and custom
    /// `messages` endpoints) are asked to keep thinking off for speed via
    /// the `thinking: {"type": "disabled"}` request field; newer Claude
    /// models enable adaptive thinking by default, and the explicit field
    /// guards MiniMax-M3 against a default change.
    disable_thinking: bool,
}

impl RigClient {
    pub fn from_config(config: &ProviderConfig, provider_id: ProviderId) -> Result<Self> {
        let meta = ProviderMeta::for_provider(provider_id);
        let api_key = meta.resolve_api_key(config.api_key());
        // Configs saved before Google retired gemini-2.5-flash now 404
        // ("Please update your code to use models/gemini-3.6-flash");
        // silently follow the replacement instead of failing at runtime.
        let model = if provider_id == ProviderId::Google
            && crate::provider::GOOGLE_RETIRED_MODELS.contains(&config.model())
        {
            crate::provider::GOOGLE_RETIRED_MODEL_REPLACEMENT.to_string()
        } else {
            config.model().to_string()
        };
        let base_url = config.base_url().or(meta.default_base_url);

        if meta.requires_api_key && !meta.api_key_optional && api_key.is_none() {
            return Err(AppError::ProviderConfig(format!(
                "Provider {provider_id:?} requires an API key"
            )));
        }

        let api_key = api_key.unwrap_or_default();
        let openai_native = provider_id == ProviderId::OpenAi;
        // Custom OpenAI-compatible gateways (Aliyun MaaS, DashScope, ...)
        // default many of their models to thinking mode, which burns the
        // token budget on reasoning and multiplies latency. The server
        // always sends thinking controls to custom providers
        // (open-course-server `openai_chat_body`); the CLI defaults them
        // the same way unless the config overrides either field. Named
        // providers keep the plain request by default: some reject unknown
        // fields, and the server only adds controls on retry there.
        let custom_openai = provider_id == ProviderId::Custom && config.endpoint() != "messages";
        let reasoning_effort = config
            .reasoning_effort()
            .map(|s| s.to_string())
            .or_else(|| custom_openai.then(|| "low".to_string()));
        // OpenAI rejects unknown parameters with a 400, and enable_thinking
        // is a Qwen/Aliyun extension — drop it for the real OpenAI API.
        let enable_thinking = if openai_native {
            None
        } else {
            config.enable_thinking().or(custom_openai.then_some(false))
        };

        let inner = match provider_id {
            ProviderId::Anthropic => {
                let base_url = base_url.unwrap_or("https://api.anthropic.com");
                let client = anthropic::Client::builder()
                    .api_key(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                RigClientInner::Anthropic(client)
            }
            ProviderId::Google => {
                let base_url = base_url.unwrap_or("https://generativelanguage.googleapis.com");
                let client = gemini::Client::builder()
                    .api_key(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                RigClientInner::Gemini(client)
            }
            ProviderId::Custom if config.endpoint() == "messages" => {
                let base_url = base_url.ok_or_else(|| {
                    AppError::ProviderConfig(format!(
                        "Provider {provider_id:?} requires a base URL"
                    ))
                })?;
                let anthropic_base = base_url.trim_end_matches("/v1").trim_end_matches('/');
                let client = anthropic::Client::builder()
                    .api_key(&api_key)
                    .base_url(anthropic_base)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                RigClientInner::Anthropic(client)
            }
            ProviderId::MiniMax => {
                // Rows saved before the move to the Anthropic-compatible
                // API still carry the old OpenAI-compatible default; treat
                // it as unset so they follow the new default.
                let base_url = match config.base_url().map(|u| u.trim_end_matches('/')) {
                    None | Some("") | Some(crate::provider::MINIMAX_LEGACY_BASE_URL) => {
                        crate::provider::MINIMAX_DEFAULT_BASE_URL
                    }
                    Some(custom) => custom,
                };
                let client = anthropic::Client::builder()
                    .api_key(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                RigClientInner::Anthropic(client)
            }
            _ => {
                let base_url = base_url.ok_or_else(|| {
                    AppError::ProviderConfig(format!(
                        "Provider {provider_id:?} requires a base URL"
                    ))
                })?;
                let client = openai::CompletionsClient::builder()
                    .api_key(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                RigClientInner::OpenAi(client)
            }
        };

        let disable_thinking = matches!(inner, RigClientInner::Anthropic(_));

        Ok(Self {
            inner,
            model,
            reasoning_effort,
            enable_thinking,
            disable_thinking,
        })
    }

    /// Extra request fields for OpenAI-compatible providers on the rig
    /// (non-streaming) paths: the Qwen/Aliyun `enable_thinking` toggle and
    /// the `reasoning_effort` control. The manual streaming body carries
    /// the same fields (`streaming::build_openai_request_body`).
    fn openai_additional_params(&self) -> Option<serde_json::Value> {
        let mut params = serde_json::Map::new();
        if self.enable_thinking == Some(false) {
            params.insert("enable_thinking".to_string(), serde_json::json!(false));
        }
        if let Some(effort) = &self.reasoning_effort {
            params.insert("reasoning_effort".to_string(), serde_json::json!(effort));
        }
        if params.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(params))
        }
    }

    pub(crate) async fn extract_typed_impl<
        T: DeserializeOwned + JsonSchema + Send + Sync + Serialize + 'static,
    >(
        &self,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<T> {
        let mut last_err = None;
        let mut native_supported = true;
        for attempt in 1..=LLM_MAX_RETRIES {
            if native_supported {
                match self.prompt_typed_native::<T>(prompt, max_tokens).await {
                    Ok(value) => return Ok(value),
                    Err(e) => {
                        let retry_after = e.retry_after();
                        let app_err = classify_llm_error(e);
                        if matches!(app_err, AppError::ProviderUnavailable(_)) {
                            if attempt < LLM_MAX_RETRIES {
                                tokio::time::sleep(retry_delay(attempt, retry_after)).await;
                                last_err = Some(app_err);
                                continue;
                            }
                            return Err(app_err);
                        }
                        // A non-transient failure (e.g. the gateway rejects
                        // response_format/output_config, or the constrained
                        // output did not deserialize): degrade to the
                        // submit-tool extractor for this and the remaining
                        // attempts instead of failing the extraction.
                        crate::debug_log::log_debug_event(
                            "extract",
                            &format!(
                                "Native structured output failed ({app_err}), falling back to tool extraction"
                            ),
                            None,
                        );
                        native_supported = false;
                    }
                }
            }

            let result = match &self.inner {
                RigClientInner::OpenAi(client) => {
                    let mut extractor = ExtractorBuilder::<T>::new(
                        openai::completion::CompletionModel::new(client.clone(), &self.model),
                    )
                    .max_tokens(max_tokens as u64);
                    if let Some(params) = self.openai_additional_params() {
                        extractor = extractor.additional_params(params);
                    }
                    extractor.build().extract(prompt).await
                }
                RigClientInner::Anthropic(client) => {
                    let mut extractor = client
                        .extractor::<T>(&self.model)
                        .max_tokens(max_tokens as u64);
                    if self.disable_thinking {
                        extractor =
                            extractor.additional_params(anthropic_disable_thinking_params());
                    }
                    extractor.build().extract(prompt).await
                }
                RigClientInner::Gemini(client) => {
                    let mut extractor = client
                        .extractor::<T>(&self.model)
                        .max_tokens(max_tokens as u64);
                    if let Some(params) =
                        ProviderMeta::for_provider(ProviderId::Google).rig_additional_params()
                    {
                        extractor = extractor.additional_params(params);
                    }
                    extractor.build().extract(prompt).await
                }
            };

            match result {
                Ok(value) => return Ok(value),
                Err(e) => {
                    let retry_after = e.retry_after();
                    let app_err = classify_llm_error(e);
                    if matches!(app_err, AppError::ProviderUnavailable(_))
                        && attempt < LLM_MAX_RETRIES
                    {
                        tokio::time::sleep(retry_delay(attempt, retry_after)).await;
                        last_err = Some(app_err);
                        continue;
                    }
                    return Err(app_err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            AppError::Llm("Failed to extract structured response after retries".to_string())
        }))
    }

    /// Native structured output: the provider is constrained by the JSON
    /// schema of `T` (OpenAI `response_format`, Anthropic `output_config`,
    /// Gemini `response_json_schema`) and rig deserializes the answer.
    async fn prompt_typed_native<T: DeserializeOwned + JsonSchema + Send + 'static>(
        &self,
        prompt: &str,
        max_tokens: u32,
    ) -> std::result::Result<T, StructuredOutputError> {
        match &self.inner {
            RigClientInner::OpenAi(client) => {
                Self::openai_agent(
                    client,
                    &self.model,
                    None,
                    max_tokens,
                    self.openai_additional_params(),
                )
                .prompt_typed::<T>(prompt)
                .await
            }
            RigClientInner::Anthropic(client) => {
                Self::anthropic_agent(client, &self.model, None, max_tokens, self.disable_thinking)
                    .prompt_typed::<T>(prompt)
                    .await
            }
            RigClientInner::Gemini(client) => {
                Self::gemini_agent(client, &self.model, None, max_tokens)
                    .prompt_typed::<T>(prompt)
                    .await
            }
        }
    }

    fn openai_agent(
        client: &openai::CompletionsClient,
        model: &str,
        system: Option<&str>,
        max_tokens: u32,
        additional_params: Option<serde_json::Value>,
    ) -> Agent {
        let builder =
            openai::completion::CompletionModel::new(client.clone(), model).into_agent_builder();
        let builder = builder.max_tokens(max_tokens as u64);
        let builder = if let Some(params) = additional_params {
            builder.additional_params(params)
        } else {
            builder
        };
        let builder = if let Some(system) = system {
            builder.preamble(system)
        } else {
            builder
        };
        builder.build()
    }

    fn anthropic_agent(
        client: &anthropic::Client,
        model: &str,
        system: Option<&str>,
        max_tokens: u32,
        disable_thinking: bool,
    ) -> Agent {
        let mut builder = client.agent(model).max_tokens(max_tokens as u64);
        if disable_thinking {
            builder = builder.additional_params(anthropic_disable_thinking_params());
        }
        if let Some(system) = system {
            builder = builder.preamble(system);
        }
        builder.build()
    }

    fn gemini_agent(
        client: &gemini::Client,
        model: &str,
        system: Option<&str>,
        max_tokens: u32,
    ) -> Agent {
        let mut builder = client.agent(model).max_tokens(max_tokens as u64);
        if let Some(params) = ProviderMeta::for_provider(ProviderId::Google).rig_additional_params()
        {
            builder = builder.additional_params(params);
        }
        if let Some(system) = system {
            builder = builder.preamble(system);
        }
        builder.build()
    }

    /// One-shot streaming prompt over any rig completion model: same
    /// preamble/max_tokens/additional_params wiring as the agent builders,
    /// with rig's native SSE handling behind it.
    async fn stream_model<M>(
        model: M,
        system: Option<&str>,
        prompt: &str,
        max_tokens: u32,
        additional_params: Option<serde_json::Value>,
    ) -> Result<LlmStream>
    where
        M: CompletionModel + Clone + 'static,
    {
        let mut builder = model
            .completion_request(prompt)
            .max_tokens(max_tokens as u64);
        if let Some(system) = system {
            builder = builder.preamble(system.to_string());
        }
        if let Some(params) = additional_params {
            builder = builder.additional_params(params);
        }
        let response = builder.stream().await.map_err(classify_llm_error)?;
        Ok(crate::streaming::into_llm_stream(response))
    }
}

#[async_trait]
impl LlmClient for RigClient {
    async fn prompt(&self, prompt: &str, system: Option<&str>, max_tokens: u32) -> Result<String> {
        let mut last_err = None;
        for attempt in 1..=LLM_MAX_RETRIES {
            let result = match &self.inner {
                RigClientInner::OpenAi(client) => {
                    Self::openai_agent(
                        client,
                        &self.model,
                        system,
                        max_tokens,
                        self.openai_additional_params(),
                    )
                    .prompt(prompt)
                    .await
                }
                RigClientInner::Anthropic(client) => {
                    Self::anthropic_agent(
                        client,
                        &self.model,
                        system,
                        max_tokens,
                        self.disable_thinking,
                    )
                    .prompt(prompt)
                    .await
                }
                RigClientInner::Gemini(client) => {
                    Self::gemini_agent(client, &self.model, system, max_tokens)
                        .prompt(prompt)
                        .await
                }
            };

            match result {
                Ok(text) => return Ok(text),
                Err(e) => {
                    let retry_after = e.retry_after();
                    let app_err = classify_llm_error(e);
                    if matches!(app_err, AppError::ProviderUnavailable(_))
                        && attempt < LLM_MAX_RETRIES
                    {
                        tokio::time::sleep(retry_delay(attempt, retry_after)).await;
                        last_err = Some(app_err);
                        continue;
                    }
                    return Err(app_err);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| AppError::Llm("Failed to prompt model after retries".to_string())))
    }

    async fn stream_prompt(
        &self,
        prompt: &str,
        system: Option<&str>,
        max_tokens: u32,
    ) -> Result<LlmStream> {
        match &self.inner {
            RigClientInner::OpenAi(client) => {
                let model = openai::completion::CompletionModel::new(client.clone(), &self.model);
                Self::stream_model(
                    model,
                    system,
                    prompt,
                    max_tokens,
                    self.openai_additional_params(),
                )
                .await
            }
            RigClientInner::Anthropic(client) => {
                let model = client.completion_model(self.model.clone());
                let params = self
                    .disable_thinking
                    .then(anthropic_disable_thinking_params);
                Self::stream_model(model, system, prompt, max_tokens, params).await
            }
            RigClientInner::Gemini(client) => {
                let model = client.completion_model(self.model.clone());
                let params = ProviderMeta::for_provider(ProviderId::Google).rig_additional_params();
                Self::stream_model(model, system, prompt, max_tokens, params).await
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env_var<F: FnOnce()>(name: &str, value: Option<&str>, f: F) {
        let _guard = crate::env_test_lock::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let original = std::env::var(name).ok();
        unsafe {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        f();
        unsafe {
            match original {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }

    fn config(
        api_key: Option<&str>,
        base_url: Option<&str>,
        endpoint: Option<&str>,
    ) -> ProviderConfig {
        ProviderConfig::ApiKey {
            api_key: api_key.map(str::to_string),
            model: "test-model".to_string(),
            base_url: base_url.map(str::to_string),
            endpoint: endpoint.map(str::to_string),
            reasoning_effort: None,
            enable_thinking: None,
        }
    }

    /// Base URL the constructed rig client will actually call — what the
    /// removed `RigClient::base_url` field used to record.
    fn inner_base_url(client: &RigClient) -> &str {
        match &client.inner {
            RigClientInner::OpenAi(c) => c.base_url(),
            RigClientInner::Anthropic(c) => c.base_url(),
            RigClientInner::Gemini(c) => c.base_url(),
        }
    }

    #[test]
    fn missing_api_key_without_env_var_errors() {
        with_env_var("ANTHROPIC_API_KEY", None, || {
            let cfg = config(None, None, None);
            let result = RigClient::from_config(&cfg, ProviderId::Anthropic);
            assert!(result.is_err());
        });
    }

    #[test]
    fn env_var_fallback_allows_construction_without_configured_key() {
        with_env_var("ANTHROPIC_API_KEY", Some("env-anthropic-key"), || {
            let cfg = config(None, None, None);
            RigClient::from_config(&cfg, ProviderId::Anthropic)
                .expect("should fall back to env var");
        });
    }

    #[test]
    fn custom_provider_without_base_url_errors() {
        with_env_var("OPENAI_API_KEY", None, || {
            let cfg = config(Some("key"), None, None);
            let result = RigClient::from_config(&cfg, ProviderId::Custom);
            assert!(result.is_err());
        });
    }

    #[test]
    fn custom_openai_provider_disables_thinking_by_default() {
        // Mirrors the server: custom OpenAI-compatible gateways always get
        // thinking controls, because their models often default to thinking
        // mode (Aliyun MaaS, DashScope).
        let cfg = config(Some("key"), Some("https://example.com/v1"), None);
        let client = RigClient::from_config(&cfg, ProviderId::Custom).expect("should build");
        assert_eq!(client.enable_thinking, Some(false));
        assert_eq!(client.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(
            client.openai_additional_params(),
            Some(serde_json::json!({
                "enable_thinking": false,
                "reasoning_effort": "low",
            }))
        );
    }

    #[test]
    fn custom_openai_provider_respects_config_overrides() {
        let mut cfg = config(Some("key"), Some("https://example.com/v1"), None);
        let ProviderConfig::ApiKey {
            enable_thinking,
            reasoning_effort,
            ..
        } = &mut cfg;
        *enable_thinking = Some(true);
        *reasoning_effort = Some("high".to_string());
        let client = RigClient::from_config(&cfg, ProviderId::Custom).expect("should build");
        assert_eq!(client.enable_thinking, Some(true));
        assert_eq!(client.reasoning_effort.as_deref(), Some("high"));
        // enable_thinking=true is the provider default anyway; only the
        // reasoning_effort override is forwarded.
        assert_eq!(
            client.openai_additional_params(),
            Some(serde_json::json!({ "reasoning_effort": "high" }))
        );
    }

    #[test]
    fn custom_messages_endpoint_gets_no_openai_thinking_defaults() {
        let cfg = config(
            Some("key"),
            Some("https://example.com/anthropic"),
            Some("messages"),
        );
        let client = RigClient::from_config(&cfg, ProviderId::Custom).expect("should build");
        assert!(matches!(client.inner, RigClientInner::Anthropic(_)));
        assert!(client.disable_thinking);
        assert_eq!(client.enable_thinking, None);
        assert_eq!(client.reasoning_effort, None);
    }

    #[test]
    fn named_openai_compatible_providers_get_no_thinking_defaults() {
        // Named providers keep plain requests by default (some reject
        // unknown fields); the server only adds thinking controls on retry
        // there, so config remains the opt-in.
        let cfg = config(Some("key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::DeepSeek).expect("should build");
        assert_eq!(client.enable_thinking, None);
        assert_eq!(client.reasoning_effort, None);
        assert_eq!(client.openai_additional_params(), None);
    }

    #[test]
    fn google_builds_with_default_base_url() {
        let cfg = config(Some("gemini-key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::Google).expect("should build");
        assert_eq!(
            inner_base_url(&client),
            "https://generativelanguage.googleapis.com"
        );
        assert!(matches!(client.inner, RigClientInner::Gemini(_)));
    }

    #[test]
    fn openai_drops_enable_thinking() {
        let mut cfg = config(Some("openai-key"), None, None);
        let ProviderConfig::ApiKey {
            enable_thinking, ..
        } = &mut cfg;
        *enable_thinking = Some(false);
        let client = RigClient::from_config(&cfg, ProviderId::OpenAi).expect("should build");
        assert_eq!(client.enable_thinking, None);

        // Custom gateways keep the configured enable_thinking.
        let mut custom_cfg = config(Some("key"), Some("https://example.com/v1"), None);
        let ProviderConfig::ApiKey {
            enable_thinking, ..
        } = &mut custom_cfg;
        *enable_thinking = Some(false);
        let custom = RigClient::from_config(&custom_cfg, ProviderId::Custom).expect("should build");
        assert_eq!(custom.enable_thinking, Some(false));
    }

    #[test]
    fn minimax_uses_anthropic_api_with_thinking_disabled() {
        with_env_var("MINIMAX_API_KEY", None, || {
            let cfg = config(Some("minimax-key"), None, None);
            let client = RigClient::from_config(&cfg, ProviderId::MiniMax).expect("should build");
            assert_eq!(
                inner_base_url(&client),
                crate::provider::MINIMAX_DEFAULT_BASE_URL
            );
            assert!(matches!(client.inner, RigClientInner::Anthropic(_)));
            assert!(client.disable_thinking);
        });
    }

    #[test]
    fn anthropic_uses_thinking_disabled() {
        let cfg = config(Some("anthropic-key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::Anthropic).expect("should build");
        assert!(matches!(client.inner, RigClientInner::Anthropic(_)));
        assert!(client.disable_thinking);
    }

    #[test]
    fn custom_messages_endpoint_uses_thinking_disabled() {
        let mut cfg = config(Some("key"), Some("https://example.com/v1"), None);
        let ProviderConfig::ApiKey { endpoint, .. } = &mut cfg;
        *endpoint = Some("messages".to_string());
        let client = RigClient::from_config(&cfg, ProviderId::Custom).expect("should build");
        assert!(matches!(client.inner, RigClientInner::Anthropic(_)));
        assert!(client.disable_thinking);
    }

    #[test]
    fn openai_compatible_providers_keep_thinking_untouched() {
        let cfg = config(Some("key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::DeepSeek).expect("should build");
        assert!(!client.disable_thinking);
    }

    #[test]
    fn minimax_legacy_base_url_migrates_to_anthropic_endpoint() {
        with_env_var("MINIMAX_API_KEY", None, || {
            for stored in [
                None,
                Some("https://api.minimax.io/v1"),
                Some("https://api.minimax.io/v1/"),
            ] {
                let cfg = config(Some("minimax-key"), stored, None);
                let client =
                    RigClient::from_config(&cfg, ProviderId::MiniMax).expect("should build");
                assert_eq!(
                    inner_base_url(&client),
                    crate::provider::MINIMAX_DEFAULT_BASE_URL,
                    "stored base_url {stored:?}"
                );
            }
            // A genuinely custom base URL is kept.
            let cfg = config(
                Some("minimax-key"),
                Some("https://proxy.example.com/anthropic"),
                None,
            );
            let client = RigClient::from_config(&cfg, ProviderId::MiniMax).expect("should build");
            assert_eq!(
                inner_base_url(&client),
                "https://proxy.example.com/anthropic"
            );
        });
    }

    #[test]
    fn other_providers_have_no_additional_params_by_default() {
        let cfg = config(Some("key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::DeepSeek).expect("should build");
        assert_eq!(client.openai_additional_params(), None);
    }

    #[test]
    fn google_retired_models_remap_to_replacement() {
        for retired in crate::provider::GOOGLE_RETIRED_MODELS {
            let mut cfg = config(Some("gemini-key"), None, None);
            let ProviderConfig::ApiKey { model, .. } = &mut cfg;
            *model = retired.to_string();
            let client = RigClient::from_config(&cfg, ProviderId::Google).expect("should build");
            assert_eq!(
                client.model,
                crate::provider::GOOGLE_RETIRED_MODEL_REPLACEMENT,
                "stored model {retired:?}"
            );
        }
    }

    #[test]
    fn google_other_models_are_untouched() {
        for kept in ["gemini-3.6-flash", "gemini-3-pro", "test-model"] {
            let mut cfg = config(Some("gemini-key"), None, None);
            let ProviderConfig::ApiKey { model, .. } = &mut cfg;
            *model = kept.to_string();
            let client = RigClient::from_config(&cfg, ProviderId::Google).expect("should build");
            assert_eq!(client.model, kept, "model {kept:?}");
        }
    }

    #[test]
    fn retired_google_model_ids_do_not_affect_other_providers() {
        // A non-Google provider configured with a retired Google model id
        // (e.g. an OpenAI-compatible gateway proxying Gemini) keeps it.
        let mut cfg = config(Some("key"), Some("https://proxy.example.com/v1"), None);
        let ProviderConfig::ApiKey { model, .. } = &mut cfg;
        *model = "gemini-2.5-flash".to_string();
        let client = RigClient::from_config(&cfg, ProviderId::Custom).expect("should build");
        assert_eq!(client.model, "gemini-2.5-flash");
    }

    #[test]
    fn schema_rejection_is_not_classified_as_unavailable() {
        // A gateway rejecting response_format/output_config must downgrade
        // to the submit-tool extractor, not burn the transient-retry budget.
        let err = classify_llm_error("ProviderError: 400 response_format is not supported");
        assert!(matches!(err, AppError::Llm(_)));
    }

    #[test]
    fn provider_overload_stays_retryable() {
        let err = classify_llm_error("server_error: provider overloaded");
        assert!(matches!(err, AppError::ProviderUnavailable(_)));
    }

    fn http_error_with_retry_after(
        status: http::StatusCode,
        body: &str,
        retry_after: &str,
    ) -> CompletionError {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::RETRY_AFTER,
            http::HeaderValue::from_str(retry_after).unwrap(),
        );
        CompletionError::from_http_response(status, body)
            .with_response_headers(Some(Box::new(headers)))
    }

    #[test]
    fn typed_429_is_unavailable_and_carries_retry_after() {
        let err = http_error_with_retry_after(
            http::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"message":"rate limited"}}"#,
            "7",
        );
        assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));
        match classify_llm_error(err) {
            AppError::ProviderUnavailable(msg) => assert!(msg.contains("rate limited"), "{msg}"),
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn typed_5xx_is_unavailable_with_provider_message_and_request_id() {
        let err = CompletionError::from_http_response_with_request_id(
            http::StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":{"message":"model overloaded"}}"#,
            Some("req_abc123".to_string()),
        );
        match classify_llm_error(err) {
            AppError::ProviderUnavailable(msg) => {
                assert!(msg.contains("model overloaded"), "{msg}");
                assert!(msg.contains("req_abc123"), "{msg}");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn typed_auth_failure_is_not_retryable() {
        for status in [http::StatusCode::UNAUTHORIZED, http::StatusCode::FORBIDDEN] {
            let err =
                CompletionError::from_http_response(status, r#"{"error":{"message":"bad key"}}"#);
            match classify_llm_error(err) {
                AppError::Llm(msg) => {
                    assert!(msg.contains("authentication failed"), "{msg}");
                    assert!(msg.contains("bad key"), "{msg}");
                }
                other => panic!("expected AppError::Llm for {status}, got {other:?}"),
            }
        }
    }

    #[test]
    fn typed_400_is_not_retryable() {
        let err = CompletionError::from_http_response(
            http::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"response_format is not supported"}}"#,
        );
        match classify_llm_error(err) {
            AppError::Llm(msg) => assert!(msg.contains("response_format"), "{msg}"),
            other => panic!("expected AppError::Llm, got {other:?}"),
        }
    }

    #[test]
    fn typed_error_data_survives_wrapper_errors() {
        let completion = CompletionError::from_http_response_with_request_id(
            http::StatusCode::BAD_GATEWAY,
            "bad gateway",
            Some("req_wrap".to_string()),
        );
        let prompt_err = PromptError::CompletionError(completion);
        match classify_llm_error(prompt_err) {
            AppError::ProviderUnavailable(msg) => assert!(msg.contains("req_wrap"), "{msg}"),
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }

        let completion =
            CompletionError::from_http_response(http::StatusCode::SERVICE_UNAVAILABLE, "down");
        let extraction = ExtractionError::CompletionError(completion);
        assert!(matches!(
            classify_llm_error(extraction),
            AppError::ProviderUnavailable(_)
        ));
    }

    #[test]
    fn string_classification_applies_without_typed_status() {
        // Plain-string errors (no typed data) still use the substring
        // heuristic, e.g. rig-generated diagnostics and connect failures.
        let err = classify_llm_error("HttpError: error trying to connect: dns error");
        assert!(matches!(err, AppError::Llm(_)));
    }

    #[test]
    fn retry_delay_prefers_capped_retry_after_hint() {
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(3600))),
            MAX_RETRY_AFTER
        );
        assert_eq!(retry_delay(2, None), Duration::from_millis(1000));
    }
}
