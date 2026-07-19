use ratatui::crossterm::event::KeyCode;

use crate::app::{AppState, LlmResult, View};
use crate::config::OpenCourseConfig;
use crate::config::provider::{ProviderConfig, ProviderId};
use crate::config::write_config;
use crate::error::{AppError, Result};
use crate::llm::provider::ProviderMeta;
use crate::ui::views::model_check;
use crate::ui::widgets::model_picker::{self, ModelPickerAction, ModelPickerOptions};

use super::{Section, SettingsState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderSetupStep {
    #[default]
    SelectProvider,
    BaseUrl,
    Endpoint,
    ApiKey,
    Model,
}

impl SettingsState {
    pub(super) fn load_provider_setup_input(&mut self, config: &OpenCourseConfig) {
        let provider = self.provider_setup_provider;
        let provider_config = config.providers.get(&provider);
        let meta = ProviderMeta::for_provider(provider);

        self.input = match self.provider_setup_step {
            ProviderSetupStep::SelectProvider => provider.as_str().to_string(),
            ProviderSetupStep::BaseUrl => {
                if provider == ProviderId::Custom {
                    provider_config
                        .and_then(|p| p.base_url().map(|s| s.to_string()))
                        .unwrap_or_default()
                } else {
                    meta.default_base_url.unwrap_or("").to_string()
                }
            }
            ProviderSetupStep::Endpoint => {
                if provider == ProviderId::Custom {
                    provider_config
                        .map(|p| p.endpoint().to_string())
                        .unwrap_or_else(|| "chat/completions".to_string())
                } else {
                    endpoint_for_known_provider(provider).to_string()
                }
            }
            ProviderSetupStep::ApiKey => provider_config
                .and_then(|p| p.api_key().map(|s| s.to_string()))
                .unwrap_or_default(),
            ProviderSetupStep::Model => provider_config
                .map(|p| p.model().to_string())
                .unwrap_or_default(),
        };
    }
}

pub(super) fn build_provider_setup_body(state: &AppState, config: &OpenCourseConfig) -> String {
    let provider = state.settings.provider_setup_provider;
    let _provider_config = config.providers.get(&provider);
    let meta = ProviderMeta::for_provider(provider);

    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => {
            let mut lines = vec!["Select provider:".to_string()];
            for p in ProviderId::all() {
                let marker = if *p == provider { "> " } else { "  " };
                lines.push(format!("{}{} - {}", marker, p.as_str(), p.label()));
            }
            lines.join("\n")
        }
        ProviderSetupStep::BaseUrl => {
            if provider == ProviderId::Custom {
                format!("Base URL: {}", state.settings.input)
            } else {
                format!(
                    "Base URL: {} (read-only)",
                    meta.default_base_url.unwrap_or("(none)")
                )
            }
        }
        ProviderSetupStep::Endpoint => {
            if provider == ProviderId::Custom {
                format!(
                    "Endpoint: {}\n\nChoose the API endpoint path for your custom provider:\n\n  chat/completions  — OpenAI-compatible endpoints\n                    (OpenCode Go: DeepSeek, GLM, Kimi, MiMo; Ollama; OpenRouter)\n  messages          — Anthropic Messages API\n                    (OpenCode Go: Qwen, MiniMax; Anthropic)",
                    state.settings.input
                )
            } else {
                format!(
                    "Endpoint: {} (read-only)\n\nThis provider uses the {} endpoint.",
                    state.settings.input, state.settings.input
                )
            }
        }
        ProviderSetupStep::ApiKey => {
            let masked = "*".repeat(state.settings.input.chars().count());
            match meta.env_key {
                Some(name) if state.settings.input.is_empty() => format!(
                    "API key: {}\n\nLeave empty to use the {} environment variable{}",
                    masked,
                    name,
                    if std::env::var(name).is_ok() {
                        " (currently set)"
                    } else {
                        " (not currently set)"
                    }
                ),
                _ => format!("API key: {}", masked),
            }
        }
        ProviderSetupStep::Model => {
            if state.settings.model_picker.loading {
                "Loading models...".to_string()
            } else if let Some(err) = &state.settings.model_picker.error {
                format!(
                    "Error loading models: {}\n\nEnter: enter manually | r: retry | Esc: back",
                    err
                )
            } else if state.settings.model_picker.manual {
                format!("Model (manual): {}", state.settings.input)
            } else {
                "No models loaded.\nEnter: enter manually".to_string()
            }
        }
    }
}

pub(super) fn build_provider_setup_footer(state: &AppState) -> String {
    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => "↑/↓: navigate | Enter: select | Esc: back",
        ProviderSetupStep::BaseUrl => {
            if state.settings.provider_setup_provider == ProviderId::Custom {
                "Enter: save | Esc: back"
            } else {
                "Enter: next | Esc: back"
            }
        }
        ProviderSetupStep::Endpoint => {
            if state.settings.provider_setup_provider == ProviderId::Custom {
                "Enter: save | Esc: back"
            } else {
                "Enter: next | Esc: back"
            }
        }
        ProviderSetupStep::ApiKey => "Enter: save | Esc: back",
        ProviderSetupStep::Model => {
            if state.settings.model_picker.loading {
                "Esc: back"
            } else if state.settings.model_picker.error.is_some() {
                "Enter: manual | r: retry | Esc: back"
            } else if state.settings.model_picker.manual {
                "Enter: save | Esc: back"
            } else if state.settings.model_picker.models.is_empty() {
                "Enter: enter manually | Esc: back"
            } else {
                "↑/↓: navigate | Enter: select | Esc: back"
            }
        }
    }
    .to_string()
}

pub fn spawn_provider_model_load(state: &mut AppState) {
    let provider = state.settings.provider_setup_provider;
    let Some(provider_config) = state
        .config
        .as_ref()
        .and_then(|config| config.providers.get(&provider))
        .cloned()
    else {
        return;
    };

    let meta = ProviderMeta::for_provider(provider);
    model_picker::spawn_load(
        &mut state.settings.model_picker,
        state.llm_tx.clone(),
        provider,
        meta.resolve_api_key(provider_config.api_key()),
        provider_config.base_url().map(|s| s.to_string()),
        LlmResult::Models,
    );
}

pub fn jump_to_model_selection(state: &mut AppState) {
    let Some(config) = state.config.as_ref() else {
        return;
    };
    state.view = View::Settings;
    state.settings.section = Section::Provider;
    state.settings.in_section = true;
    state.settings.provider_setup_step = ProviderSetupStep::Model;
    state.settings.provider_setup_provider = config.active_provider;
    state.settings.model_picker.reset();
    state.settings.loaded_field = None;
    state.settings.input = config
        .providers
        .get(&config.active_provider)
        .map(|p| p.model().to_string())
        .unwrap_or_default();
    spawn_provider_model_load(state);
}

pub(super) fn init_provider_setup(state: &mut AppState) {
    let Some(config) = state.config.as_ref() else {
        return;
    };
    let provider = config.active_provider;
    state.settings.provider_setup_step = ProviderSetupStep::SelectProvider;
    state.settings.provider_setup_provider = provider;
    state.settings.model_picker.reset();
    state.settings.model_picker.loading = false;
    state.settings.loaded_field = None;
    state.settings.input = provider.as_str().to_string();
}

fn ensure_provider_config(config: &mut OpenCourseConfig, provider_id: ProviderId) {
    if config.providers.contains_key(&provider_id) {
        return;
    }
    let default_url = ProviderMeta::for_provider(provider_id)
        .default_base_url
        .map(|s| s.to_string());
    config.providers.insert(
        provider_id,
        ProviderConfig::ApiKey {
            api_key: None,
            model: String::new(),
            base_url: default_url,
            endpoint: None,
            reasoning_effort: None,
        },
    );
}

fn endpoint_for_known_provider(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Anthropic => "messages",
        ProviderId::Google => "generative-language",
        _ => "chat/completions",
    }
}

pub fn advance_provider_setup_step(state: &mut AppState) {
    let provider = state.settings.provider_setup_provider;
    let meta = ProviderMeta::for_provider(provider);
    let next = match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::BaseUrl
            } else if meta.requires_api_key {
                ProviderSetupStep::ApiKey
            } else {
                ProviderSetupStep::Model
            }
        }
        ProviderSetupStep::BaseUrl => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else if meta.requires_api_key {
                ProviderSetupStep::ApiKey
            } else {
                ProviderSetupStep::Model
            }
        }
        ProviderSetupStep::Endpoint => {
            if meta.requires_api_key {
                ProviderSetupStep::ApiKey
            } else {
                ProviderSetupStep::Model
            }
        }
        ProviderSetupStep::ApiKey => ProviderSetupStep::Model,
        ProviderSetupStep::Model => {
            state.settings.in_section = false;
            return;
        }
    };
    state.settings.provider_setup_step = next;
    state.settings.loaded_field = None;
    if let Some(config) = state.config.as_ref() {
        state.settings.load_provider_setup_input(config);
    }
    if state.settings.provider_setup_step == ProviderSetupStep::Model
        && state.settings.model_picker.models.is_empty()
        && !state.settings.model_picker.loading
    {
        spawn_provider_model_load(state);
    }
}

fn go_back_provider_setup_step(state: &mut AppState) {
    let provider = state.settings.provider_setup_provider;
    let meta = ProviderMeta::for_provider(provider);
    let prev = match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => {
            state.settings.in_section = false;
            return;
        }
        ProviderSetupStep::BaseUrl => ProviderSetupStep::SelectProvider,
        ProviderSetupStep::Endpoint => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::BaseUrl
            } else {
                ProviderSetupStep::SelectProvider
            }
        }
        ProviderSetupStep::ApiKey => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else {
                ProviderSetupStep::SelectProvider
            }
        }
        ProviderSetupStep::Model => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else if meta.requires_api_key {
                ProviderSetupStep::ApiKey
            } else {
                ProviderSetupStep::SelectProvider
            }
        }
    };
    state.settings.provider_setup_step = prev;
    state.settings.loaded_field = None;
    if let Some(config) = state.config.as_ref() {
        state.settings.load_provider_setup_input(config);
    }
}

pub(super) async fn handle_provider_setup_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => handle_select_provider_step(state, code).await,
        ProviderSetupStep::BaseUrl => handle_base_url_step(state, code).await,
        ProviderSetupStep::Endpoint => handle_endpoint_step(state, code).await,
        ProviderSetupStep::ApiKey => handle_api_key_step(state, code).await,
        ProviderSetupStep::Model => handle_model_step(state, code).await,
    }
}

async fn handle_select_provider_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    let all = ProviderId::all();
    let current = all
        .iter()
        .position(|p| *p == state.settings.provider_setup_provider)
        .unwrap_or(0);
    match code {
        KeyCode::Esc => {
            state.settings.in_section = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let new_idx = (current + all.len() - 1) % all.len();
            state.settings.provider_setup_provider = all[new_idx];
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let new_idx = (current + 1) % all.len();
            state.settings.provider_setup_provider = all[new_idx];
        }
        KeyCode::Enter => {
            if let Some(config) = state.config.as_mut() {
                let selected = state.settings.provider_setup_provider;
                config.active_provider = selected;
                ensure_provider_config(config, selected);
                state.settings.model_picker.reset();
                if let Err(e) = write_config(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                } else {
                    state.settings.error = None;
                    advance_provider_setup_step(state);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_base_url_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            go_back_provider_setup_step(state);
        }
        KeyCode::Enter => {
            if let Some(config) = state.config.as_mut() {
                let provider = state.settings.provider_setup_provider;
                if provider == ProviderId::Custom {
                    let value = state.settings.input.trim().to_string();
                    if value.is_empty() {
                        state.settings.error =
                            Some("Base URL is required for custom provider".to_string());
                        return Ok(());
                    }
                    if let Some(provider_config) = config.providers.get(&provider) {
                        let updated = provider_config.clone().with_base_url(Some(value));
                        config.providers.insert(provider, updated);
                    }
                }
                if let Err(e) = write_config(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                } else {
                    state.settings.error = None;
                    advance_provider_setup_step(state);
                }
            }
        }
        KeyCode::Char(c) if state.settings.provider_setup_provider == ProviderId::Custom => {
            state.settings.input.push(c);
        }
        KeyCode::Backspace if state.settings.provider_setup_provider == ProviderId::Custom => {
            state.settings.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_endpoint_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            go_back_provider_setup_step(state);
        }
        KeyCode::Enter => {
            if let Some(config) = state.config.as_mut() {
                let provider = state.settings.provider_setup_provider;
                if provider == ProviderId::Custom {
                    let value = state.settings.input.trim().to_string();
                    if !value.eq("chat/completions") && !value.eq("messages") {
                        state.settings.error =
                            Some("Endpoint must be 'chat/completions' or 'messages'".to_string());
                        return Ok(());
                    }
                    if let Some(provider_config) = config.providers.get(&provider) {
                        let updated = provider_config.clone().with_endpoint(Some(value));
                        config.providers.insert(provider, updated);
                    }
                }
                if let Err(e) = write_config(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                } else {
                    state.settings.error = None;
                    advance_provider_setup_step(state);
                }
            }
        }
        KeyCode::Char(c) if state.settings.provider_setup_provider == ProviderId::Custom => {
            state.settings.input.push(c);
        }
        KeyCode::Backspace if state.settings.provider_setup_provider == ProviderId::Custom => {
            state.settings.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_api_key_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            go_back_provider_setup_step(state);
        }
        KeyCode::Enter => {
            if let Some(config) = state.config.as_mut() {
                let provider = state.settings.provider_setup_provider;
                let meta = ProviderMeta::for_provider(provider);
                let value = state.settings.input.trim().to_string();
                if meta.requires_api_key
                    && !meta.api_key_optional
                    && value.is_empty()
                    && meta.resolve_api_key(None).is_none()
                {
                    let hint = meta
                        .env_key
                        .map(|name| format!(" or set the {name} environment variable"))
                        .unwrap_or_default();
                    state.settings.error =
                        Some(format!("API key is required for this provider{hint}"));
                    return Ok(());
                }
                if let Some(provider_config) = config.providers.get(&provider) {
                    let updated = provider_config.clone().with_api_key(if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    });
                    config.providers.insert(provider, updated);
                }
                if let Err(e) = write_config(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                } else {
                    state.settings.error = None;
                    advance_provider_setup_step(state);
                }
            }
        }
        KeyCode::Char(c) => {
            state.settings.input.push(c);
        }
        KeyCode::Backspace => {
            state.settings.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_model_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    let action = model_picker::handle_key(
        &mut state.settings.model_picker,
        &mut state.settings.input,
        code,
        &ModelPickerOptions::SETTINGS,
    );
    match action {
        ModelPickerAction::Ignored
        | ModelPickerAction::InputPushed
        | ModelPickerAction::InputPopped => {}
        ModelPickerAction::Back => go_back_provider_setup_step(state),
        ModelPickerAction::Retry => spawn_provider_model_load(state),
        ModelPickerAction::EnterManual => {
            state.settings.model_picker.error = None;
            if let Some(config) = state.config.as_ref() {
                state.settings.load_provider_setup_input(config);
            }
        }
        ModelPickerAction::ExitManual => {
            state.settings.model_picker.error = None;
            if state.settings.model_picker.models.is_empty() {
                state.settings.model_picker.error = Some("No models loaded".to_string());
            }
        }
        ModelPickerAction::EmptyEnter => {
            state.settings.model_picker.manual = true;
            if let Some(config) = state.config.as_ref() {
                state.settings.load_provider_setup_input(config);
            }
        }
        ModelPickerAction::ConfirmManual => {
            let value = state.settings.input.trim().to_string();
            if let Err(e) = save_model_and_run_diagnostics(state, value) {
                state.settings.error = Some(e.to_string());
            }
        }
        ModelPickerAction::Select(model_id) => {
            if let Err(e) = save_model_and_run_diagnostics(state, model_id) {
                state.settings.error = Some(e.to_string());
            }
        }
    }
    Ok(())
}

fn save_model_and_run_diagnostics(state: &mut AppState, model_id: String) -> Result<()> {
    if model_id.is_empty() {
        return Err(AppError::Config("Model is required".to_string()));
    }
    let provider = state.settings.provider_setup_provider;
    let config_clone = {
        let config = state
            .config
            .as_mut()
            .ok_or(AppError::Config("No config".to_string()))?;
        let provider_config = config
            .providers
            .get(&provider)
            .ok_or(AppError::Config("Provider config not found".to_string()))?;
        let updated = provider_config.clone().with_model(model_id);
        config.providers.insert(provider, updated);
        write_config(config, &state.data_dir)?;
        config.clone()
    };
    state.settings.error = None;
    state.settings.in_section = true;
    state.settings.provider_setup_step = ProviderSetupStep::Model;
    state.settings.model_picker.manual = false;
    state.settings.loaded_field = None;
    model_check::start(state, config_clone, View::Settings);
    Ok(())
}
