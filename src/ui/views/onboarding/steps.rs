use crate::app::AppState;
use crate::config::provider::ProviderId;
use crate::db::curriculum::CEFR_LEVELS;
use crate::llm::provider::ProviderMeta;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    NativeLanguage,
    TargetLanguage,
    Age,
    Cefr,
    BatchSize,
    Provider,
    ApiKey,
    BaseUrl,
    Model,
}

impl Step {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Step::NativeLanguage => "Native language (e.g. en)",
            Step::TargetLanguage => "Target language (e.g. es)",
            Step::Age => "Age (optional)",
            Step::Cefr => "CEFR level (required)",
            Step::BatchSize => "Batch size (required)",
            Step::Provider => "Select provider",
            Step::ApiKey => "Enter API key",
            Step::BaseUrl => "Enter base URL",
            Step::Model => "Select model",
        }
    }

    pub(super) fn all() -> &'static [Step] {
        &[
            Step::NativeLanguage,
            Step::TargetLanguage,
            Step::Age,
            Step::Cefr,
            Step::BatchSize,
            Step::Provider,
            Step::ApiKey,
            Step::BaseUrl,
            Step::Model,
        ]
    }
}

pub(super) const BATCH_SIZES: &[&str] = &["2", "3", "4", "5"];

pub(super) fn base_url_default(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Custom => "https://opencode.ai/zen/go/v1",
        ProviderId::Ollama => "http://localhost:11434/v1",
        _ => "",
    }
}

pub(super) fn shows_base_url_step(provider: ProviderId) -> bool {
    matches!(provider, ProviderId::Custom | ProviderId::Ollama)
}

pub(super) fn is_text_step(step: Step) -> bool {
    matches!(
        step,
        Step::NativeLanguage
            | Step::TargetLanguage
            | Step::Age
            | Step::ApiKey
            | Step::BaseUrl
    )
}

pub(super) fn step_help_text(step: Step, state: &AppState) -> String {
    match step {
        Step::Provider => provider_help(state),
        Step::ApiKey => api_key_help(state),
        Step::BaseUrl => base_url_help(state),
        Step::NativeLanguage => {
            "Enter your native language code (ISO 639-1, e.g. en, ru)".to_string()
        }
        Step::TargetLanguage => {
            "Enter the language you want to learn (ISO 639-1, e.g. es, de)".to_string()
        }
        Step::Age => "Enter your age (optional, used to pick age-appropriate contexts)".to_string(),
        Step::Cefr => cefr_help(state),
        Step::BatchSize => batch_size_help(state),
        Step::Model => String::new(),
    }
}

fn provider_help(state: &AppState) -> String {
    let mut text = String::from("Available providers:\n");
    for p in ProviderId::all() {
        let marker = if *p == state.onboarding.provider {
            "> "
        } else {
            "  "
        };
        text.push_str(&format!("{}{} - {}\n", marker, p.as_str(), p.label()));
    }
    text
}

fn api_key_help(state: &AppState) -> String {
    let meta = ProviderMeta::for_provider(state.onboarding.provider);
    let env_note = match meta.env_key {
        Some(name) if std::env::var(name).is_ok() => {
            format!("\n{name} is set in your environment and will be used if you leave this blank.")
        }
        Some(name) => format!("\nYou can also set the {name} environment variable instead."),
        None => String::new(),
    };
    if meta.requires_api_key && !meta.api_key_optional {
        format!(
            "Enter API key for {}\n(required){}",
            state.onboarding.provider.label(),
            env_note
        )
    } else {
        format!(
            "Enter API key for {}\n(optional — press Enter to skip){}",
            state.onboarding.provider.label(),
            env_note
        )
    }
}

fn base_url_help(state: &AppState) -> String {
    if shows_base_url_step(state.onboarding.provider) {
        format!(
            "Enter API base URL for {}\n(e.g. {})",
            state.onboarding.provider.label(),
            base_url_default(state.onboarding.provider)
        )
    } else {
        format!(
            "Base URL is not required for {}.\nPress Enter to continue.",
            state.onboarding.provider.label()
        )
    }
}

fn cefr_help(state: &AppState) -> String {
    let mut text = String::from(
        "Select your CEFR level (required). Pick the level that best matches your current ability (self-assessment):\n",
    );
    for level in CEFR_LEVELS {
        let marker = if *level == state.onboarding.cefr {
            "> "
        } else {
            "  "
        };
        text.push_str(&format!("{}{}\n", marker, level));
    }
    text
}

fn batch_size_help(state: &AppState) -> String {
    let mut text =
        String::from("Select batch size — number of exercises per session (required):\n");
    for size in BATCH_SIZES {
        let marker = if *size == state.onboarding.batch_size.to_string().as_str() {
            "> "
        } else {
            "  "
        };
        text.push_str(&format!("{}{}\n", marker, size));
    }
    text
}
