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

use crate::config::provider::{ProviderConfig, ProviderId};
use crate::error::{AppError, Result};
use crate::llm::provider::ProviderMeta;
use crate::llm::streaming::LlmStream;

const LLM_MAX_RETRIES: usize = 3;

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

#[async_trait]
pub trait LlmClient: Send + Sync + Any {
    async fn prompt(&self, prompt: &str, system: Option<&str>) -> Result<String>;
    async fn stream_prompt(&self, prompt: &str, system: Option<&str>) -> Result<LlmStream>;

    fn as_any(&self) -> &dyn Any;
}

impl dyn LlmClient {
    pub async fn extract<T: DeserializeOwned + JsonSchema + Send + Sync + Serialize + 'static>(
        &self,
        prompt: &str,
    ) -> Result<T> {
        let client = self
            .as_any()
            .downcast_ref::<RigClient>()
            .ok_or_else(|| AppError::Llm("Unsupported LLM client implementation".to_string()))?;
        client.extract_typed::<T>(prompt).await
    }
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
}

impl RigClient {
    pub fn from_config(config: &ProviderConfig, provider_id: ProviderId) -> Result<Self> {
        let meta = ProviderMeta::for_provider(provider_id);
        let api_key = config.api_key();
        let model = config.model().to_string();
        let base_url = config.base_url().or(meta.default_base_url);

        if meta.requires_api_key && !meta.api_key_optional && api_key.is_none() {
            return Err(AppError::ProviderConfig(format!(
                "Provider {provider_id:?} requires an API key"
            )));
        }

        let api_key = api_key.unwrap_or_default().to_string();
        let reasoning_effort = config.reasoning_effort().map(|s| s.to_string());

        let (inner, base_url) = match provider_id {
            ProviderId::Anthropic => {
                let base_url = base_url.unwrap_or("https://api.anthropic.com");
                let client = anthropic::ClientBuilder::new(&api_key)
                    .base_url(base_url)
                    .build();
                (RigClientInner::Anthropic(client), base_url.to_string())
            }
            ProviderId::Google => {
                let base_url = base_url.unwrap_or("https://generativelanguage.googleapis.com");
                let client = gemini::Client::from_url(&api_key, base_url);
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
                    .build();
                (
                    RigClientInner::Anthropic(client),
                    anthropic_base.to_string(),
                )
            }
            _ => {
                let base_url = base_url.ok_or_else(|| {
                    AppError::ProviderConfig(format!(
                        "Provider {provider_id:?} requires a base URL"
                    ))
                })?;
                let client = openai::Client::from_url(&api_key, base_url);
                (RigClientInner::OpenAi(client), base_url.to_string())
            }
        };

        Ok(Self {
            inner,
            model,
            base_url,
            api_key,
            reasoning_effort,
        })
    }

    async fn extract_typed<T: DeserializeOwned + JsonSchema + Send + Sync + Serialize + 'static>(
        &self,
        prompt: &str,
    ) -> Result<T> {
        let mut last_err = None;
        for attempt in 1..=LLM_MAX_RETRIES {
            let result = match &self.inner {
                RigClientInner::OpenAi(client) => {
                    let extractor = ExtractorBuilder::<T, _>::new(
                        openai::completion::CompletionModel::new(client.clone(), &self.model),
                    )
                    .max_tokens(8192)
                    .build();
                    extractor.extract(prompt).await
                }
                RigClientInner::Anthropic(client) => {
                    client
                        .extractor::<T>(&self.model)
                        .max_tokens(8192)
                        .build()
                        .extract(prompt)
                        .await
                }
                RigClientInner::Gemini(client) => {
                    client
                        .extractor::<T>(&self.model)
                        .build()
                        .extract(prompt)
                        .await
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
    ) -> Agent<openai::completion::CompletionModel> {
        let builder =
            openai::completion::CompletionModel::new(client.clone(), model).into_agent_builder();
        let builder = builder.max_tokens(8192);
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
    ) -> Agent<anthropic::completion::CompletionModel> {
        let mut builder = client.agent(model).max_tokens(8192);
        if let Some(system) = system {
            builder = builder.preamble(system);
        }
        builder.build()
    }

    fn gemini_agent(
        client: &gemini::Client,
        model: &str,
        system: Option<&str>,
    ) -> Agent<gemini::completion::CompletionModel> {
        let mut builder = client.agent(model);
        if let Some(system) = system {
            builder = builder.preamble(system);
        }
        builder.build()
    }
}

#[async_trait]
impl LlmClient for RigClient {
    async fn prompt(&self, prompt: &str, system: Option<&str>) -> Result<String> {
        let mut last_err = None;
        for attempt in 1..=LLM_MAX_RETRIES {
            let result = match &self.inner {
                RigClientInner::OpenAi(client) => {
                    Self::openai_agent(client, &self.model, system)
                        .prompt(prompt)
                        .await
                }
                RigClientInner::Anthropic(client) => {
                    Self::anthropic_agent(client, &self.model, system)
                        .prompt(prompt)
                        .await
                }
                RigClientInner::Gemini(client) => {
                    Self::gemini_agent(client, &self.model, system)
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

    async fn stream_prompt(&self, prompt: &str, system: Option<&str>) -> Result<LlmStream> {
        match &self.inner {
            RigClientInner::OpenAi(_) => {
                crate::llm::streaming::stream_openai_compatible(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    system,
                    prompt,
                    self.reasoning_effort.as_deref(),
                )
                .await
            }
            RigClientInner::Anthropic(_) => {
                crate::llm::streaming::stream_anthropic_messages(
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    system,
                    prompt,
                )
                .await
            }
            RigClientInner::Gemini(_) => {
                let text = self.prompt(prompt, system).await?;
                Ok(crate::llm::streaming::stream_from_text(text))
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
