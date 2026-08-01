use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

use crate::app::AppState;
use crate::ui::colors;
use crate::ui::labels::{OnboardingLabels, get_onboarding_labels, native_language_code};
use open_course_config::provider::ProviderId;
use open_course_db::curriculum::CEFR_LEVELS;
use open_course_llm::provider::ProviderMeta;

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
    pub(super) fn label(&self, lang: &str) -> &'static str {
        let labels = get_onboarding_labels(lang);
        match self {
            Step::NativeLanguage => labels.step_native_language,
            Step::TargetLanguage => labels.step_target_language,
            Step::Age => labels.step_age,
            Step::Cefr => labels.step_cefr,
            Step::BatchSize => labels.step_batch_size,
            Step::Provider => labels.step_provider,
            Step::ApiKey => labels.step_api_key,
            Step::BaseUrl => labels.step_base_url,
            Step::Model => labels.step_model,
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
    let labels = get_onboarding_labels(native_language_code(state.config.as_ref()));
    match step {
        Step::ApiKey => api_key_help(state, labels),
        Step::BaseUrl => base_url_help(state, labels),
        Step::NativeLanguage => labels.help_native_language.to_string(),
        Step::TargetLanguage => labels.help_target_language.to_string(),
        Step::Age => labels.help_age.to_string(),
        _ => String::new(),
    }
}

/// Options list for selector steps (provider, CEFR, batch size) with the
/// currently selected option highlighted in the design-system green.
pub(super) fn step_selector_text(step: Step, state: &AppState) -> Text<'static> {
    let labels = get_onboarding_labels(native_language_code(state.config.as_ref()));
    let intro = match step {
        Step::Provider => labels.intro_providers,
        Step::Cefr => labels.intro_cefr,
        Step::BatchSize => labels.intro_batch_size,
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
            .map(|level| ((*level).to_string(), *level == state.onboarding.cefr))
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

fn api_key_help(state: &AppState, labels: OnboardingLabels) -> String {
    let meta = ProviderMeta::for_provider(state.onboarding.provider);
    let env_note = match meta.env_key {
        Some(name) if std::env::var(name).is_ok() => {
            format!("\n{}", labels.env_var_set.replace("{name}", name))
        }
        Some(name) => format!("\n{}", labels.env_var_hint.replace("{name}", name)),
        None => String::new(),
    };
    let template = if meta.requires_api_key && !meta.api_key_optional {
        labels.api_key_required
    } else {
        labels.api_key_optional
    };
    template
        .replacen("{}", state.onboarding.provider.label(), 1)
        .replacen("{}", &env_note, 1)
}

fn base_url_help(state: &AppState, labels: OnboardingLabels) -> String {
    if shows_base_url_step(state.onboarding.provider) {
        labels
            .base_url_prompt
            .replacen("{}", state.onboarding.provider.label(), 1)
            .replacen("{}", base_url_default(state.onboarding.provider), 1)
    } else {
        labels
            .base_url_not_required
            .replacen("{}", state.onboarding.provider.label(), 1)
    }
}
