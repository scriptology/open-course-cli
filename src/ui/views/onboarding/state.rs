use crate::config::provider::ProviderId;
use crate::core::language::{is_valid_language_code, normalize_language_code};
use crate::db::curriculum::CEFR_LEVELS;
use crate::error::{AppError, Result};
use crate::ui::widgets::model_picker::ModelPickerState;

use super::steps::{Step, base_url_default, shows_base_url_step};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnboardingMode {
    #[default]
    Initial,
    AddPair,
}

#[derive(Debug, Clone)]
pub struct OnboardingState {
    pub mode: OnboardingMode,
    pub steps: Vec<Step>,
    pub active: usize,
    pub input: String,
    pub provider: ProviderId,
    pub provider_index: usize,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub native_language: String,
    pub target_language: String,
    pub age: String,
    pub cefr: String,
    pub batch_size: u32,
    pub error: String,
    pub model_picker: ModelPickerState,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self::new()
    }
}

impl OnboardingState {
    pub fn new() -> Self {
        Self::for_mode(OnboardingMode::Initial)
    }

    pub fn for_add_pair() -> Self {
        Self::for_mode(OnboardingMode::AddPair)
    }

    fn for_mode(mode: OnboardingMode) -> Self {
        let steps = match mode {
            OnboardingMode::Initial => Step::all().to_vec(),
            OnboardingMode::AddPair => vec![
                Step::NativeLanguage,
                Step::TargetLanguage,
                Step::Age,
                Step::Cefr,
            ],
        };
        let default_provider = ProviderId::all()[0];
        Self {
            mode,
            steps,
            active: 0,
            input: String::new(),
            provider: default_provider,
            provider_index: 0,
            model: String::new(),
            api_key: String::new(),
            base_url: base_url_default(default_provider).to_string(),
            native_language: String::new(),
            target_language: String::new(),
            age: String::new(),
            cefr: "A1".to_string(),
            batch_size: 3,
            error: String::new(),
            model_picker: ModelPickerState::default(),
        }
    }

    pub(super) fn current_step(&self) -> Step {
        self.steps[self.active]
    }

    pub(super) fn is_step_visible(&self, step: Step) -> bool {
        match step {
            Step::BaseUrl => shows_base_url_step(self.provider),
            _ => true,
        }
    }

    pub(super) fn go_forward(&mut self) {
        while self.active < self.steps.len() - 1 {
            self.active += 1;
            if self.is_step_visible(self.current_step()) {
                break;
            }
        }
        self.load_input();
    }

    pub(super) fn go_back(&mut self) {
        while self.active > 0 {
            self.active -= 1;
            if self.is_step_visible(self.current_step()) {
                break;
            }
        }
        self.load_input();
    }

    pub(super) fn set_provider(&mut self, provider: ProviderId) {
        if self.base_url.is_empty() || self.base_url == base_url_default(self.provider) {
            self.base_url = base_url_default(provider).to_string();
        }
        self.provider = provider;
        self.provider_index = ProviderId::all()
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(ProviderId::all().len().saturating_sub(1));
    }

    pub(super) fn load_input(&mut self) {
        self.input = match self.current_step() {
            Step::Provider => self.provider.as_str().to_string(),
            Step::ApiKey => self.api_key.clone(),
            Step::BaseUrl => self.base_url.clone(),
            Step::Model => self.model.clone(),
            Step::NativeLanguage => self.native_language.clone(),
            Step::TargetLanguage => self.target_language.clone(),
            Step::Age => self.age.clone(),
            Step::Cefr => self.cefr.clone(),
            Step::BatchSize => self.batch_size.to_string(),
        };
        if self.current_step() == Step::Provider {
            self.provider_index = ProviderId::all()
                .iter()
                .position(|p| *p == self.provider)
                .unwrap_or(ProviderId::all().len().saturating_sub(1));
        }
        self.error.clear();
    }

    pub(super) fn apply_input(&mut self) -> Result<()> {
        let value = self.input.trim().to_string();
        match self.current_step() {
            Step::Provider => {
                let norm = value.to_lowercase();
                if let Some(p) = ProviderId::all().iter().find(|p| p.as_str() == norm) {
                    self.provider = *p;
                }
            }
            Step::ApiKey => self.api_key = value,
            Step::BaseUrl => {
                if shows_base_url_step(self.provider) && value.is_empty() {
                    return Err(AppError::Config(
                        "Base URL is required for this provider".to_string(),
                    ));
                }
                self.base_url = value;
            }
            Step::Model => {
                if value.is_empty() {
                    return Err(AppError::Config("Model is required".to_string()));
                }
                self.model = value;
            }
            Step::NativeLanguage => {
                let norm = normalize_language_code(&value);
                if !is_valid_language_code(&norm) {
                    return Err(AppError::Config(format!("Invalid language code: {value}")));
                }
                self.native_language = norm;
            }
            Step::TargetLanguage => {
                let norm = normalize_language_code(&value);
                if !is_valid_language_code(&norm) {
                    return Err(AppError::Config(format!("Invalid language code: {value}")));
                }
                if norm == self.native_language {
                    return Err(AppError::Config(
                        "Target language must differ from native language".to_string(),
                    ));
                }
                self.target_language = norm;
            }
            Step::Age => {
                if value.is_empty() {
                    self.age = String::new();
                } else {
                    match value.parse::<u32>() {
                        Ok(age) if (1..=120).contains(&age) => self.age = age.to_string(),
                        _ => {
                            return Err(AppError::Config(format!(
                                "Age must be a number between 1 and 120: {value}"
                            )));
                        }
                    }
                }
            }
            Step::Cefr => {
                if value.is_empty() {
                    return Err(AppError::Config("CEFR level is required".to_string()));
                }
                if !CEFR_LEVELS.contains(&value.to_uppercase().as_str()) {
                    return Err(AppError::Config(format!("Invalid CEFR level: {value}")));
                }
                self.cefr = value.to_uppercase();
            }
            Step::BatchSize => {
                if value.is_empty() {
                    return Err(AppError::Config("Batch size is required".to_string()));
                }
                match value.parse::<u32>() {
                    Ok(n) if (2..=5).contains(&n) => self.batch_size = n,
                    _ => return Err(AppError::Config(format!("Batch size must be 2-5: {value}"))),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_key_step_index(state: &OnboardingState) -> usize {
        state.steps.iter().position(|s| *s == Step::ApiKey).unwrap()
    }

    fn base_url_step_index(state: &OnboardingState) -> usize {
        state.steps.iter().position(|s| *s == Step::BaseUrl).unwrap()
    }

    fn model_step_index(state: &OnboardingState) -> usize {
        state.steps.iter().position(|s| *s == Step::Model).unwrap()
    }

    #[test]
    fn defaults_to_first_provider() {
        let state = OnboardingState::new();
        assert_eq!(state.provider, ProviderId::all()[0]);
        assert_eq!(state.provider_index, 0);
    }

    #[test]
    fn set_provider_does_not_leak_stale_base_url() {
        let mut state = OnboardingState::new();
        state.set_provider(ProviderId::Custom);
        assert_eq!(state.base_url, base_url_default(ProviderId::Custom));

        state.set_provider(ProviderId::Google);
        assert_eq!(state.base_url, base_url_default(ProviderId::Google));
        assert!(state.base_url.is_empty());
    }

    #[test]
    fn set_provider_keeps_user_edited_base_url() {
        let mut state = OnboardingState::new();
        state.set_provider(ProviderId::Custom);
        state.base_url = "https://my-custom-endpoint.example".to_string();

        state.set_provider(ProviderId::Ollama);
        assert_eq!(state.base_url, "https://my-custom-endpoint.example");
    }

    #[test]
    fn base_url_step_visible_only_for_custom_and_ollama() {
        let mut state = OnboardingState::new();
        for provider in ProviderId::all() {
            state.provider = *provider;
            assert_eq!(
                state.is_step_visible(Step::BaseUrl),
                shows_base_url_step(*provider)
            );
        }
    }

    #[test]
    fn other_steps_are_always_visible() {
        let state = OnboardingState::new();
        for step in Step::all() {
            if *step != Step::BaseUrl {
                assert!(state.is_step_visible(*step));
            }
        }
    }

    #[test]
    fn go_forward_skips_base_url_when_not_required() {
        let mut state = OnboardingState::new();
        state.set_provider(ProviderId::Google);
        state.active = api_key_step_index(&state);

        state.go_forward();

        assert_eq!(state.active, model_step_index(&state));
    }

    #[test]
    fn go_forward_stops_on_base_url_when_required() {
        let mut state = OnboardingState::new();
        state.set_provider(ProviderId::Custom);
        state.active = api_key_step_index(&state);

        state.go_forward();

        assert_eq!(state.active, base_url_step_index(&state));
    }

    #[test]
    fn go_back_skips_base_url_when_not_required() {
        let mut state = OnboardingState::new();
        state.set_provider(ProviderId::Google);
        state.active = model_step_index(&state);

        state.go_back();

        assert_eq!(state.active, api_key_step_index(&state));
    }

    #[test]
    fn go_back_from_first_step_is_a_no_op() {
        let mut state = OnboardingState::new();
        state.active = 0;

        state.go_back();

        assert_eq!(state.active, 0);
    }
}
