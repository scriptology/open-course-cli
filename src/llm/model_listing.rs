use std::time::Duration;

use crate::config::provider::ProviderId;
use crate::error::{AppError, Result};
use crate::llm::provider::ProviderMeta;

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
        ProviderId::Anthropic => list_anthropic_models(api_key, base_url).await,
        ProviderId::Google => list_gemini_models(api_key, base_url).await,
        _ => list_openai_compatible_models(provider_id, api_key, base_url).await,
    }
}

async fn list_anthropic_models(
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelInfo>> {
    let base_url = base_url
        .unwrap_or("https://api.anthropic.com")
        .trim_end_matches('/');
    let url = format!("{base_url}/v1/models");
    let api_key = api_key.ok_or_else(|| {
        AppError::ProviderConfig("Anthropic requires an API key to list models".to_string())
    })?;

    let client = http_client()?;
    let response = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to fetch Anthropic models: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Llm(format!(
            "Anthropic model listing returned {status}: {body}"
        )));
    }

    let payload: AnthropicModelList = response
        .json()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to parse Anthropic model list: {e}")))?;

    let models: Vec<ModelInfo> = payload
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            label: Some(m.display_name),
        })
        .collect();
    Ok(models)
}

async fn list_gemini_models(
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelInfo>> {
    let base_url = base_url
        .unwrap_or("https://generativelanguage.googleapis.com")
        .trim_end_matches('/');
    let api_key = api_key.ok_or_else(|| {
        AppError::ProviderConfig("Gemini requires an API key to list models".to_string())
    })?;
    let url = format!("{base_url}/v1beta/models?key={api_key}");

    let client = http_client()?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to fetch Gemini models: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Llm(format!(
            "Gemini model listing returned {status}: {body}"
        )));
    }

    let payload: GeminiModelList = response
        .json()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to parse Gemini model list: {e}")))?;

    let models: Vec<ModelInfo> = payload
        .models
        .into_iter()
        .map(|m| {
            let id = m
                .name
                .strip_prefix("models/")
                .unwrap_or(&m.name)
                .to_string();
            ModelInfo {
                id,
                label: Some(m.display_name),
            }
        })
        .collect();
    Ok(models)
}

async fn list_openai_compatible_models(
    provider_id: ProviderId,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelInfo>> {
    let meta = ProviderMeta::for_provider(provider_id);
    let base_url = base_url
        .or(meta.default_base_url)
        .ok_or_else(|| AppError::ProviderConfig(format!("{provider_id:?} requires a base URL")))?
        .trim_end_matches('/');
    let url = format!("{base_url}/models");

    let client = http_client()?;
    let mut request = client.get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to fetch models: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Llm(format!(
            "Model listing returned {status}: {body}"
        )));
    }

    let payload: OpenAiModelList = response
        .json()
        .await
        .map_err(|e| AppError::Llm(format!("Failed to parse model list: {e}")))?;

    let models: Vec<ModelInfo> = payload
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            label: None,
        })
        .collect();
    Ok(models)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Llm(format!("Failed to build HTTP client: {e}")))
}

#[derive(Debug, serde::Deserialize)]
struct AnthropicModelList {
    data: Vec<AnthropicModel>,
}

#[derive(Debug, serde::Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: String,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiModelList {
    models: Vec<GeminiModel>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiModel {
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiModel {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_builds() {
        let _client = http_client().unwrap();
    }
}
