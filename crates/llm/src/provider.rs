use open_course_config::provider::ProviderId;

/// MiniMax's Anthropic-compatible API: MiniMax-M3 keeps thinking off by
/// default there, unlike the OpenAI-compatible endpoint where reasoning
/// burns the completion budget and the answer comes back empty.
pub const MINIMAX_DEFAULT_BASE_URL: &str = "https://api.minimax.io/anthropic";

/// MiniMax's OpenAI-compatible API: the pre-Anthropic chat default. Chat
/// configs carrying it are migrated to `MINIMAX_DEFAULT_BASE_URL`; model
/// listing still uses it (the `/anthropic` path serves messages only).
pub const MINIMAX_LEGACY_BASE_URL: &str = "https://api.minimax.io/v1";

/// Google model ids retired from the Gemini API: requests 404 with
/// "Please update your code to use models/gemini-3.6-flash". Configs still
/// carrying one of these ids are remapped to `GOOGLE_RETIRED_MODEL_REPLACEMENT`
/// at client construction time.
pub const GOOGLE_RETIRED_MODELS: [&str; 2] = ["gemini-2.5-flash", "models/gemini-2.5-flash"];

/// Replacement served for every id in `GOOGLE_RETIRED_MODELS`.
pub const GOOGLE_RETIRED_MODEL_REPLACEMENT: &str = "gemini-3.6-flash";

/// True for OpenAI model families that reason by default and accept
/// `reasoning_effort` (gpt-5 and up, and the o-series); other OpenAI
/// models reject the field outright with a 400, so it must only be sent
/// to these families. Mirrors rig's internal `is_openai_reasoning_model`
/// classification (same families that require `max_completion_tokens`).
pub fn is_openai_reasoning_model(model: &str) -> bool {
    let numbered_gpt_5_plus = model
        .strip_prefix("gpt-")
        .and_then(|rest| rest.split(['.', '-']).next())
        .filter(|major| major.len() == 1)
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 5);
    let o_series = {
        let mut chars = model.chars();
        chars.next() == Some('o')
            && chars.next().is_some_and(|c| c.is_ascii_digit())
            && chars
                .next()
                .is_none_or(|next| next == '-' || next.is_ascii_digit())
    };
    numbered_gpt_5_plus || o_series
}

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
            ProviderId::MiniMax => ProviderMeta {
                id,
                label: "MiniMax",
                requires_api_key: true,
                api_key_optional: false,
                default_base_url: Some(MINIMAX_DEFAULT_BASE_URL),
                env_key: Some("MINIMAX_API_KEY"),
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
        ProviderId::MiniMax,
        ProviderId::Ollama,
        ProviderId::Custom,
    ]
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
}
