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
        Self {
            mode,
            steps,
            active: 0,
            input: String::new(),
            provider: ProviderId::Custom,
            provider_index: ProviderId::all().len().saturating_sub(1),
            model: String::new(),
            api_key: String::new(),
            base_url: base_url_default(ProviderId::Custom).to_string(),
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

    pub(super) fn set_provider(&mut self, provider: ProviderId) {
        self.provider = provider;
        self.provider_index = ProviderId::all()
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(ProviderId::all().len().saturating_sub(1));
        if self.base_url.is_empty() || self.base_url == base_url_default(self.provider) {
            self.base_url = base_url_default(provider).to_string();
        }
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
