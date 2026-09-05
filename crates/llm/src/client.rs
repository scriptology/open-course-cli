use std::any::Any;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::extractor::ExtractorBuilder;
use rig::providers::{anthropic, gemini, openai};

use crate::provider::ProviderMeta;
use crate::streaming::LlmStream;
use open_course_config::provider::{ProviderConfig, ProviderId};
use open_course_core::error::{AppError, Result};

const LLM_MAX_RETRIES: usize = 3;

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

fn classify_llm_error<E: std::fmt::Display>(e: E) -> AppError {
    let msg = e.to_string();
    if is_provider_unavailable(&msg) {
        AppError::ProviderUnavailable(provider_error_message(&msg))
    } else {
        AppError::Llm(msg)
    }
}

/// `additional_params` that keep MiniMax-M3 thinking off on its
/// Anthropic-compatible API (M2.x models accept but ignore this).
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
    OpenAi(openai::Client),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
}

pub struct RigClient {
    inner: RigClientInner,
    model: String,
    base_url: String,
    api_key: String,
    reasoning_effort: Option<String>,
    enable_thinking: Option<bool>,
    /// True for the real OpenAI API (not OpenAI-compatible gateways):
    /// requires `max_completion_tokens` and rejects unknown parameters such
    /// as the Qwen/Aliyun `enable_thinking` extension.
    openai_native: bool,
    /// MiniMax is called through its Anthropic-compatible API where
    /// MiniMax-M3 keeps thinking off by default; the explicit
    /// `thinking: {"type": "disabled"}` additional param guards against a
    /// default change (M2.x models accept but ignore it).
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
        let reasoning_effort = config.reasoning_effort().map(|s| s.to_string());
        let openai_native = provider_id == ProviderId::OpenAi;
        // OpenAI rejects unknown parameters with a 400, and enable_thinking
        // is a Qwen/Aliyun extension — drop it for the real OpenAI API.
        let enable_thinking = if openai_native {
            None
        } else {
            config.enable_thinking()
        };

        let (inner, base_url) = match provider_id {
            ProviderId::Anthropic => {
                let base_url = base_url.unwrap_or("https://api.anthropic.com");
                let client = anthropic::ClientBuilder::new(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                (RigClientInner::Anthropic(client), base_url.to_string())
            }
            ProviderId::Google => {
                let base_url = base_url.unwrap_or("https://generativelanguage.googleapis.com");
                let client = gemini::client::ClientBuilder::new(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                (RigClientInner::Gemini(client), base_url.to_string())
            }
            ProviderId::Custom if config.endpoint() == "messages" => {
                let base_url = base_url.ok_or_else(|| {
                    AppError::ProviderConfig(format!(
                        "Provider {provider_id:?} requires a base URL"
                    ))
                })?;
                let anthropic_base = base_url.trim_end_matches("/v1").trim_end_matches('/');
                let client = anthropic::ClientBuilder::new(&api_key)
                    .base_url(anthropic_base)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                (
                    RigClientInner::Anthropic(client),
                    anthropic_base.to_string(),
                )
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
                let client = anthropic::ClientBuilder::new(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                (RigClientInner::Anthropic(client), base_url.to_string())
            }
            _ => {
                let base_url = base_url.ok_or_else(|| {
                    AppError::ProviderConfig(format!(
                        "Provider {provider_id:?} requires a base URL"
                    ))
                })?;
                let client = openai::ClientBuilder::new(&api_key)
                    .base_url(base_url)
                    .build()
                    .map_err(|e| AppError::ProviderConfig(e.to_string()))?;
                (RigClientInner::OpenAi(client), base_url.to_string())
            }
        };

        Ok(Self {
            inner,
            model,
            base_url,
            api_key,
            reasoning_effort,
            enable_thinking,
            openai_native,
            disable_thinking: provider_id == ProviderId::MiniMax,
        })
    }

    /// Extra request fields for OpenAI-compatible providers: the
    /// Qwen/Aliyun `enable_thinking` toggle (rig's `additional_params` is
    /// set once).
    fn openai_additional_params(&self) -> Option<serde_json::Value> {
        if self.enable_thinking == Some(false) {
            Some(serde_json::json!({ "enable_thinking": false }))
        } else {
            None
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
        for attempt in 1..=LLM_MAX_RETRIES {
            let result = match &self.inner {
                RigClientInner::OpenAi(client) => {
                    let mut extractor = ExtractorBuilder::<_, T>::new(
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
                    let app_err = classify_llm_error(e);
                    if matches!(app_err, AppError::ProviderUnavailable(_))
                        && attempt < LLM_MAX_RETRIES
                    {
                        tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
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

    fn openai_agent(
        client: &openai::Client,
        model: &str,
        system: Option<&str>,
        max_tokens: u32,
        additional_params: Option<serde_json::Value>,
    ) -> Agent<openai::completion::CompletionModel> {
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
    ) -> Agent<anthropic::completion::CompletionModel> {
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
    ) -> Agent<gemini::completion::CompletionModel> {
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
                    let app_err = classify_llm_error(e);
                    if matches!(app_err, AppError::ProviderUnavailable(_))
                        && attempt < LLM_MAX_RETRIES
                    {
                        tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
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
            RigClientInner::OpenAi(_) => {
                crate::streaming::stream_openai_compatible(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    system,
                    prompt,
                    self.reasoning_effort.as_deref(),
                    self.enable_thinking,
                    max_tokens,
                    self.openai_native,
                )
                .await
            }
            RigClientInner::Anthropic(_) => {
                crate::streaming::stream_anthropic_messages(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    system,
                    prompt,
                    max_tokens,
                    self.disable_thinking,
                )
                .await
            }
            RigClientInner::Gemini(_) => {
                let text = self.prompt(prompt, system, max_tokens).await?;
                Ok(crate::streaming::stream_from_text(text))
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
            let client = RigClient::from_config(&cfg, ProviderId::Anthropic)
                .expect("should fall back to env var");
            assert_eq!(client.api_key, "env-anthropic-key");
        });
    }

    #[test]
    fn configured_key_takes_priority_over_env_var() {
        with_env_var("OPENAI_API_KEY", Some("env-openai-key"), || {
            let cfg = config(
                Some("configured-key"),
                Some("https://api.openai.com/v1"),
                None,
            );
            let client = RigClient::from_config(&cfg, ProviderId::OpenAi).expect("should build");
            assert_eq!(client.api_key, "configured-key");
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
    fn google_builds_with_default_base_url() {
        let cfg = config(Some("gemini-key"), None, None);
        let client = RigClient::from_config(&cfg, ProviderId::Google).expect("should build");
        assert_eq!(client.base_url, "https://generativelanguage.googleapis.com");
        assert!(matches!(client.inner, RigClientInner::Gemini(_)));
    }

    #[test]
    fn openai_drops_enable_thinking_and_marks_native_api() {
        let mut cfg = config(Some("openai-key"), None, None);
        let ProviderConfig::ApiKey {
            enable_thinking, ..
        } = &mut cfg;
        *enable_thinking = Some(false);
        let client = RigClient::from_config(&cfg, ProviderId::OpenAi).expect("should build");
        assert!(client.openai_native);
        assert_eq!(client.enable_thinking, None);

        // Custom gateways keep the configured enable_thinking.
        let mut custom_cfg = config(Some("key"), Some("https://example.com/v1"), None);
        let ProviderConfig::ApiKey {
            enable_thinking, ..
        } = &mut custom_cfg;
        *enable_thinking = Some(false);
        let custom = RigClient::from_config(&custom_cfg, ProviderId::Custom).expect("should build");
        assert!(!custom.openai_native);
        assert_eq!(custom.enable_thinking, Some(false));
    }

    #[test]
    fn minimax_uses_anthropic_api_with_thinking_disabled() {
        with_env_var("MINIMAX_API_KEY", None, || {
            let cfg = config(Some("minimax-key"), None, None);
            let client = RigClient::from_config(&cfg, ProviderId::MiniMax).expect("should build");
            assert_eq!(client.base_url, crate::provider::MINIMAX_DEFAULT_BASE_URL);
            assert!(matches!(client.inner, RigClientInner::Anthropic(_)));
            assert!(client.disable_thinking);
        });
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
                    client.base_url,
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
            assert_eq!(client.base_url, "https://proxy.example.com/anthropic");
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
}
