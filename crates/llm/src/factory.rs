use crate::client::{LlmClient, RigClient};
use open_course_config::OpenCourseConfig;
use open_course_core::error::{AppError, Result};

pub fn create_llm_model(config: &OpenCourseConfig) -> Result<Box<dyn LlmClient>> {
    let provider_id = config.active_provider;
    let provider_config = config.providers.get(&provider_id).ok_or_else(|| {
        AppError::ProviderConfig(format!("No config for provider {provider_id:?}"))
    })?;

    let client = RigClient::from_config(provider_config, provider_id)?;
    Ok(Box::new(client))
}
