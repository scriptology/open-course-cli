use std::time::Duration;

use rig::client::ModelListingClient;
use rig::model::{Model, ModelList, ModelListingError};
use rig::providers::{anthropic, gemini, openai};

use crate::provider::ProviderMeta;
use open_course_config::provider::ProviderId;
use open_course_core::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub label: Option<String>,
}

pub async fn list_models(
    provider_id: ProviderId,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelInfo>> {
    match provider_id {
        ProviderId::Anthropic => {
            let base_url = base_url.unwrap_or("https://api.anthropic.com");
            let api_key = api_key.ok_or_else(|| {
                AppError::ProviderConfig("Anthropic requires an API key to list models".to_string())
            })?;
            let client = anthropic::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .http_client(http_client()?)
                .build()
                .map_err(|e| AppError::Llm(format!("Failed to build Anthropic client: {e}")))?;
            map_model_list(client.list_models().await)
        }
        ProviderId::Google => {
            let base_url = base_url.unwrap_or("https://generativelanguage.googleapis.com");
            let api_key = api_key.ok_or_else(|| {
                AppError::ProviderConfig("Gemini requires an API key to list models".to_string())
            })?;
            let client = gemini::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .http_client(http_client()?)
                .build()
                .map_err(|e| AppError::Llm(format!("Failed to build Gemini client: {e}")))?;
            map_model_list(client.list_models().await)
        }
        ProviderId::MiniMax => {
            // Chat moved to the Anthropic-compatible API, which serves
            // messages only; model listing stays on the OpenAI-compatible
            // endpoint. Configs carrying either known default map to it.
            let base_url = minimax_listing_base_url(base_url);
            list_openai_style_models(base_url, api_key).await
        }
        _ => {
            let meta = ProviderMeta::for_provider(provider_id);
            let base_url = base_url.or(meta.default_base_url).ok_or_else(|| {
                AppError::ProviderConfig(format!("{provider_id:?} requires a base URL"))
            })?;
            list_openai_style_models(base_url, api_key).await
        }
    }
}

/// List models from an OpenAI-style `GET {base_url}/models` endpoint via
/// rig's OpenAI completions model lister. Covers OpenAI itself plus every
/// OpenAI-compatible provider (DeepSeek, Mistral, OpenRouter, MiniMax,
/// Ollama, Custom).
async fn list_openai_style_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<ModelInfo>> {
    let client = openai::CompletionsClient::builder()
        .api_key(api_key.unwrap_or_default())
        .base_url(base_url)
        .http_client(http_client()?)
        .build()
        .map_err(|e| AppError::Llm(format!("Failed to build OpenAI client: {e}")))?;
    map_model_list(client.list_models().await)
}

fn map_model_list(
    result: std::result::Result<ModelList, ModelListingError>,
) -> Result<Vec<ModelInfo>> {
    result
        .map(|list| list.into_iter().map(model_info).collect())
        .map_err(|e| AppError::Llm(e.to_string()))
}

fn model_info(model: Model) -> ModelInfo {
    ModelInfo {
        id: model.id,
        label: model.name,
    }
}

/// Base URL for MiniMax model listing: known chat defaults (old and new)
/// map to the OpenAI-compatible endpoint, custom URLs are kept as-is.
fn minimax_listing_base_url(base_url: Option<&str>) -> &str {
    use crate::provider::{MINIMAX_DEFAULT_BASE_URL, MINIMAX_LEGACY_BASE_URL};
    match base_url.map(|u| u.trim_end_matches('/')) {
        None | Some("") | Some(MINIMAX_DEFAULT_BASE_URL) => MINIMAX_LEGACY_BASE_URL,
        Some(custom) => custom,
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Llm(format!("Failed to build HTTP client: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_builds() {
        let _client = http_client().unwrap();
    }

    #[test]
    fn minimax_listing_uses_openai_compatible_endpoint_for_known_defaults() {
        use crate::provider::{MINIMAX_DEFAULT_BASE_URL, MINIMAX_LEGACY_BASE_URL};
        assert_eq!(minimax_listing_base_url(None), MINIMAX_LEGACY_BASE_URL);
        assert_eq!(
            minimax_listing_base_url(Some(MINIMAX_DEFAULT_BASE_URL)),
            MINIMAX_LEGACY_BASE_URL
        );
        assert_eq!(
            minimax_listing_base_url(Some("https://api.minimax.io/v1/")),
            MINIMAX_LEGACY_BASE_URL
        );
        assert_eq!(
            minimax_listing_base_url(Some("https://proxy.example.com/v1")),
            "https://proxy.example.com/v1"
        );
    }

    #[test]
    fn model_info_maps_rig_model_fields() {
        let named = model_info(Model::new("claude-sonnet-5", "Claude Sonnet 5"));
        assert_eq!(named.id, "claude-sonnet-5");
        assert_eq!(named.label.as_deref(), Some("Claude Sonnet 5"));

        let anonymous = model_info(Model::from_id("gpt-5"));
        assert_eq!(anonymous.id, "gpt-5");
        assert_eq!(anonymous.label, None);
    }

    #[test]
    fn listing_errors_map_to_app_error_llm() {
        let err = map_model_list(Err(ModelListingError::api_error(401, "bad key")))
            .expect_err("should propagate");
        match err {
            AppError::Llm(msg) => {
                assert!(msg.contains("401"), "status should survive: {msg}");
                assert!(msg.contains("bad key"), "message should survive: {msg}");
            }
            other => panic!("expected AppError::Llm, got {other:?}"),
        }
    }
}
