use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

use crate::app::AppState;
use crate::config::provider::ProviderId;
use crate::db::curriculum::CEFR_LEVELS;
use crate::llm::provider::ProviderMeta;
use crate::ui::colors;

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
        Step::NativeLanguage | Step::TargetLanguage | Step::Age | Step::ApiKey | Step::BaseUrl
    )
}

pub(super) fn step_help_text(step: Step, state: &AppState) -> String {
    match step {
        Step::ApiKey => api_key_help(state),
        Step::BaseUrl => base_url_help(state),
        Step::NativeLanguage => {
            "Enter your native language code (ISO 639-1, e.g. en, ru)".to_string()
        }
        Step::TargetLanguage => {
            "Enter the language you want to learn (ISO 639-1, e.g. es, de)".to_string()
        }
        Step::Age => "Enter your age (optional, used to pick age-appropriate contexts)".to_string(),
        _ => String::new(),
    }
}

/// Options list for selector steps (provider, CEFR, batch size) with the
/// currently selected option highlighted in the design-system green.
pub(super) fn step_selector_text(step: Step, state: &AppState) -> Text<'static> {
    let intro = match step {
        Step::Provider => "Available providers:",
        Step::Cefr => "Select your CEFR level (required). Pick the level that best matches your current ability (self-assessment):",
        Step::BatchSize => {
            "Select batch size — number of exercises per session (required):"
        }
        _ => "",
    };
    let options: Vec<(String, bool)> = match step {
        Step::Provider => ProviderId::all()
            .iter()
            .map(|p| {
                (
                    format!("{} - {}", p.as_str(), p.label()),
                    *p == state.onboarding.provider,
                )
            })
            .collect(),
        Step::Cefr => CEFR_LEVELS
            .iter()
            .map(|level| {
                (
                    (*level).to_string(),
                    *level == state.onboarding.cefr,
                )
            })
            .collect(),
        Step::BatchSize => BATCH_SIZES
            .iter()
            .map(|size| {
                (
                    (*size).to_string(),
                    *size == state.onboarding.batch_size.to_string().as_str(),
                )
            })
            .collect(),
        _ => vec![],
    };

    let mut lines = vec![Line::from(intro.to_string())];
    for (label, selected) in options {
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(colors::GREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }
    Text::from(lines)
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
