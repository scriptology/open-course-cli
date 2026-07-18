use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAi,
    Anthropic,
    Google,
    DeepSeek,
    Mistral,
    OpenRouter,
    Ollama,
    Custom,
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "openai" => Ok(ProviderId::OpenAi),
            "anthropic" => Ok(ProviderId::Anthropic),
            "google" => Ok(ProviderId::Google),
            "deepseek" => Ok(ProviderId::DeepSeek),
            "mistral" => Ok(ProviderId::Mistral),
            "openrouter" => Ok(ProviderId::OpenRouter),
            "ollama" => Ok(ProviderId::Ollama),
            "custom" | "opencode" => Ok(ProviderId::Custom),
            _ => Err(serde::de::Error::custom(format!("unknown provider: {s}"))),
        }
    }
}

impl Serialize for ProviderId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderId::OpenAi => "openai",
            ProviderId::Anthropic => "anthropic",
            ProviderId::Google => "google",
            ProviderId::DeepSeek => "deepseek",
            ProviderId::Mistral => "mistral",
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Ollama => "ollama",
            ProviderId::Custom => "custom",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ProviderId::OpenAi => "OpenAI",
            ProviderId::Anthropic => "Anthropic",
            ProviderId::Google => "Google Gemini",
            ProviderId::DeepSeek => "DeepSeek",
            ProviderId::Mistral => "Mistral",
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::Ollama => "Ollama",
            ProviderId::Custom => "Custom OpenAI-compatible",
        }
    }

    pub fn requires_api_key(&self) -> bool {
        !matches!(self, ProviderId::Ollama)
    }

    pub fn api_key_optional(&self) -> bool {
        matches!(self, ProviderId::Ollama | ProviderId::Custom)
    }

    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            ProviderId::OpenRouter => Some("https://openrouter.ai/api/v1"),
            ProviderId::Ollama => Some("http://localhost:11434/v1"),
            ProviderId::Custom => None,
            _ => None,
        }
    }

    pub fn all() -> &'static [ProviderId] {
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
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProviderConfig {
    ApiKey {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(
            default = "default_endpoint",
            skip_serializing_if = "is_default_endpoint"
        )]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
    },
}

fn default_endpoint() -> Option<String> {
    Some("chat/completions".to_string())
}

fn is_default_endpoint(endpoint: &Option<String>) -> bool {
    endpoint.as_deref() == Some("chat/completions")
}

impl<'de> Deserialize<'de> for ProviderConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(rename = "type")]
            ty: String,
            #[serde(default)]
            api_key: Option<String>,
            model: String,
            #[serde(default)]
            base_url: Option<String>,
            #[serde(default = "default_endpoint")]
            endpoint: Option<String>,
            #[serde(default)]
            reasoning_effort: Option<String>,
        }
        let helper = Helper::deserialize(deserializer)?;
        match helper.ty.as_str() {
            "openCode" | "apiKey" => Ok(ProviderConfig::ApiKey {
                api_key: helper.api_key,
                model: helper.model,
                base_url: helper.base_url,
                endpoint: helper.endpoint,
                reasoning_effort: helper.reasoning_effort,
            }),
            _ => Err(serde::de::Error::custom(format!(
                "unknown provider config type: {}",
                helper.ty
            ))),
        }
    }
}

impl ProviderConfig {
    pub fn model(&self) -> &str {
        match self {
            ProviderConfig::ApiKey { model, .. } => model,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match self {
            ProviderConfig::ApiKey { api_key, .. } => api_key.as_deref(),
        }
    }

    pub fn base_url(&self) -> Option<&str> {
        match self {
            ProviderConfig::ApiKey { base_url, .. } => base_url.as_deref(),
        }
    }

    pub fn endpoint(&self) -> &str {
        match self {
            ProviderConfig::ApiKey { endpoint, .. } => {
                endpoint.as_deref().unwrap_or("chat/completions")
            }
        }
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        match self {
            ProviderConfig::ApiKey {
                reasoning_effort, ..
            } => reasoning_effort.as_deref(),
        }
    }

    /// Returns a copy with a different API key.
    pub fn with_api_key(mut self, api_key: Option<String>) -> Self {
        match &mut self {
            ProviderConfig::ApiKey { api_key: slot, .. } => *slot = api_key,
        }
        self
    }

    /// Returns a copy with a different model.
    pub fn with_model(mut self, model: String) -> Self {
        match &mut self {
            ProviderConfig::ApiKey { model: slot, .. } => *slot = model,
        }
        self
    }

    /// Returns a copy with a different base URL.
    pub fn with_base_url(mut self, base_url: Option<String>) -> Self {
        match &mut self {
            ProviderConfig::ApiKey { base_url: slot, .. } => *slot = base_url,
        }
        self
    }

    /// Returns a copy with a different endpoint.
    pub fn with_endpoint(mut self, endpoint: Option<String>) -> Self {
        match &mut self {
            ProviderConfig::ApiKey { endpoint: slot, .. } => *slot = endpoint,
        }
        self
    }
}
