use crate::config::provider::ProviderId;

pub struct ProviderMeta {
    pub id: ProviderId,
    pub label: &'static str,
    pub requires_api_key: bool,
    pub api_key_optional: bool,
    pub default_base_url: Option<&'static str>,
}

impl ProviderMeta {
    pub fn for_provider(id: ProviderId) -> Self {
        match id {
            ProviderId::OpenAi => ProviderMeta {
                id,
                label: "OpenAI",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.openai.com/v1"),
            },
            ProviderId::Anthropic => ProviderMeta {
                id,
                label: "Anthropic",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.anthropic.com"),
            },
            ProviderId::Google => ProviderMeta {
                id,
                label: "Google Gemini",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://generativelanguage.googleapis.com"),
            },
            ProviderId::DeepSeek => ProviderMeta {
                id,
                label: "DeepSeek",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.deepseek.com/v1"),
            },
            ProviderId::Mistral => ProviderMeta {
                id,
                label: "Mistral",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.mistral.ai/v1"),
            },
            ProviderId::OpenRouter => ProviderMeta {
                id,
                label: "OpenRouter",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://openrouter.ai/api/v1"),
            },
            ProviderId::Ollama => ProviderMeta {
                id,
                label: "Ollama",
                requires_api_key: false,
                api_key_optional: true,
                default_base_url: Some("http://localhost:11434/v1"),
            },
            ProviderId::Custom => ProviderMeta {
                id,
                label: "Custom OpenAI-compatible",
                requires_api_key: true,
                api_key_optional: true,
                default_base_url: None,
            },
        }
    }
}

pub fn all_providers() -> &'static [ProviderId] {
    &[
        ProviderId::OpenAi,
        ProviderId::Anthropic,
        ProviderId::Google,
        ProviderId::DeepSeek,
        ProviderId::Mistral,
        ProviderId::OpenRouter,
        ProviderId::Ollama,
        ProviderId::Custom,
    ]
}
