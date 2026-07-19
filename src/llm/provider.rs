use crate::config::provider::ProviderId;

pub struct ProviderMeta {
    pub id: ProviderId,
    pub label: &'static str,
    pub requires_api_key: bool,
    pub api_key_optional: bool,
    pub default_base_url: Option<&'static str>,
    pub env_key: Option<&'static str>,
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
                env_key: Some("OPENAI_API_KEY"),
            },
            ProviderId::Anthropic => ProviderMeta {
                id,
                label: "Anthropic",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.anthropic.com"),
                env_key: Some("ANTHROPIC_API_KEY"),
            },
            ProviderId::Google => ProviderMeta {
                id,
                label: "Google Gemini",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://generativelanguage.googleapis.com"),
                env_key: Some("GEMINI_API_KEY"),
            },
            ProviderId::DeepSeek => ProviderMeta {
                id,
                label: "DeepSeek",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.deepseek.com/v1"),
                env_key: Some("DEEPSEEK_API_KEY"),
            },
            ProviderId::Mistral => ProviderMeta {
                id,
                label: "Mistral",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://api.mistral.ai/v1"),
                env_key: Some("MISTRAL_API_KEY"),
            },
            ProviderId::OpenRouter => ProviderMeta {
                id,
                label: "OpenRouter",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some("https://openrouter.ai/api/v1"),
                env_key: Some("OPENROUTER_API_KEY"),
            },
            ProviderId::Ollama => ProviderMeta {
                id,
                label: "Ollama",
                requires_api_key: false,
                api_key_optional: true,
                default_base_url: Some("http://localhost:11434/v1"),
                env_key: None,
            },
            ProviderId::Custom => ProviderMeta {
                id,
                label: "Custom OpenAI-compatible",
                requires_api_key: true,
                api_key_optional: true,
                default_base_url: None,
                env_key: None,
            },
        }
    }

    pub fn rig_additional_params(&self) -> Option<serde_json::Value> {
        match self.id {
            ProviderId::Google => Some(serde_json::json!({ "generationConfig": {} })),
            _ => None,
        }
    }

    pub fn resolve_api_key(&self, configured: Option<&str>) -> Option<String> {
        if let Some(key) = configured
            && !key.is_empty()
        {
            return Some(key.to_string());
        }
        self.env_key
            .and_then(|name| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env_var<F: FnOnce()>(name: &str, value: Option<&str>, f: F) {
        let _guard = crate::llm::env_test_lock::LOCK
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

    #[test]
    fn resolve_api_key_prefers_configured_value() {
        let meta = ProviderMeta::for_provider(ProviderId::OpenAi);
        with_env_var("OPENAI_API_KEY", Some("env-key"), || {
            assert_eq!(
                meta.resolve_api_key(Some("configured-key")).as_deref(),
                Some("configured-key")
            );
        });
    }

    #[test]
    fn resolve_api_key_falls_back_to_env_var() {
        let meta = ProviderMeta::for_provider(ProviderId::Google);
        with_env_var("GEMINI_API_KEY", Some("env-gemini-key"), || {
            assert_eq!(
                meta.resolve_api_key(None).as_deref(),
                Some("env-gemini-key")
            );
            assert_eq!(
                meta.resolve_api_key(Some("")).as_deref(),
                Some("env-gemini-key")
            );
        });
    }

    #[test]
    fn resolve_api_key_none_when_neither_set() {
        let meta = ProviderMeta::for_provider(ProviderId::Anthropic);
        with_env_var("ANTHROPIC_API_KEY", None, || {
            assert_eq!(meta.resolve_api_key(None), None);
        });
    }

    #[test]
    fn resolve_api_key_ignores_empty_env_var() {
        let meta = ProviderMeta::for_provider(ProviderId::Mistral);
        with_env_var("MISTRAL_API_KEY", Some(""), || {
            assert_eq!(meta.resolve_api_key(None), None);
        });
    }

    #[test]
    fn providers_without_env_key_have_no_fallback() {
        let meta = ProviderMeta::for_provider(ProviderId::Ollama);
        assert_eq!(meta.resolve_api_key(None), None);
        let meta = ProviderMeta::for_provider(ProviderId::Custom);
        assert_eq!(meta.resolve_api_key(None), None);
    }

    #[test]
    fn google_requires_generation_config_workaround() {
        let meta = ProviderMeta::for_provider(ProviderId::Google);
        let params = meta
            .rig_additional_params()
            .expect("google needs additional_params");
        assert!(params.get("generationConfig").is_some());
    }

    #[test]
    fn non_google_providers_have_no_additional_params() {
        for provider in all_providers() {
            if *provider != ProviderId::Google {
                let meta = ProviderMeta::for_provider(*provider);
                assert_eq!(meta.rig_additional_params(), None);
            }
        }
    }
}
