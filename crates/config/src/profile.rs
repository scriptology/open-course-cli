use serde::{Deserialize, Serialize};

// Re-exported from core so the profile type is shared with the LLM
// prompt/parsing code and external consumers; the config file format is
// unchanged.
pub use open_course_core::profile::UserProfile;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanguagePair {
    pub id: String,
    pub profile: UserProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferences {
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_hint_mode")]
    pub hint_mode: HintMode,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            hint_mode: default_hint_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HintMode {
    #[default]
    Auto,
    OnDemand,
}

fn default_batch_size() -> u32 {
    3
}

fn default_hint_mode() -> HintMode {
    HintMode::Auto
}
