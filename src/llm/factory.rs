use crate::config::OpenCourseConfig;
use crate::error::{AppError, Result};
use crate::llm::client::{LlmClient, RigClient};

pub fn create_llm_model(config: &OpenCourseConfig) -> Result<Box<dyn LlmClient>> {
    let provider_id = config.active_provider;
    let provider_config = config.providers.get(&provider_id).ok_or_else(|| {
        AppError::ProviderConfig(format!("No config for provider {provider_id:?}"))
    })?;

    let client = RigClient::from_config(provider_config, provider_id)?;
    Ok(Box::new(client))
}
