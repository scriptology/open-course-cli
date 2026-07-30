use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, LlmResult, View};
use crate::config::OpenCourseConfig;
use crate::config::provider::{ProviderConfig, ProviderId};
use crate::config::write_config;
use crate::error::{AppError, Result};
use crate::llm::provider::ProviderMeta;
use crate::ui::colors;
use crate::ui::labels::{CommonLabels, SettingsLabels, get_settings_labels, native_language_code};
use crate::ui::views::model_check;
use crate::ui::widgets::build_footer_wrapped;
use crate::ui::widgets::model_picker::{self, ModelPickerAction, ModelPickerOptions};
use crate::ui::widgets::text_input;

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

pub(super) fn build_provider_setup_body(
    state: &AppState,
    config: &OpenCourseConfig,
    labels: &SettingsLabels,
) -> String {
    let provider = state.settings.provider_setup_provider;
    let _provider_config = config.providers.get(&provider);
    let meta = ProviderMeta::for_provider(provider);

    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => {
            let mut lines = vec![labels.select_provider_title.to_string()];
            for p in ProviderId::all() {
                let marker = if *p == provider { "> " } else { "  " };
                lines.push(format!("{}{} - {}", marker, p.as_str(), p.label()));
            }
            lines.join("\n")
        }
        // Editable BaseUrl (Custom) and ApiKey render as input boxes in
        // `draw_input_step`; the bodies below are the read-only fallbacks.
        ProviderSetupStep::BaseUrl => labels.base_url_readonly.replace(
            "{}",
            meta.default_base_url.unwrap_or(labels.none_placeholder),
        ),
        ProviderSetupStep::Endpoint => labels
            .endpoint_readonly
            .replace("{}", &state.settings.input),
        ProviderSetupStep::ApiKey => String::new(),
        ProviderSetupStep::Model => {
            if state.settings.model_picker.loading {
                labels.loading_models.to_string()
            } else if let Some(err) = &state.settings.model_picker.error {
                labels.error_loading_models.replace("{}", err)
            } else if state.settings.model_picker.manual {
                labels
                    .model_manual_label
                    .replace("{}", &state.settings.input)
            } else {
                labels.no_models_loaded.to_string()
            }
        }
    }
}

/// Renders the BaseUrl / ApiKey wizard steps as a bordered input box with a
/// block caret (same look as the onboarding inputs) plus a hint line below.
pub(super) fn draw_input_step(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    labels: &SettingsLabels,
) {
    let provider = state.settings.provider_setup_provider;
    let meta = ProviderMeta::for_provider(provider);
    let is_api_key = state.settings.provider_setup_step == ProviderSetupStep::ApiKey;

    let (title, display) = if is_api_key {
        (
            labels.api_key_title,
            "*".repeat(state.settings.input.chars().count()),
        )
    } else {
        (labels.base_url_title, state.settings.input.clone())
    };

    let hint = if is_api_key {
        match meta.env_key {
            Some(name) if state.settings.input.is_empty() => {
                let suffix = if std::env::var(name).is_ok() {
                    labels.env_currently_set
                } else {
                    labels.env_not_set
                };
                labels
                    .api_key_env_hint
                    .replacen("{}", name, 1)
                    .replacen("{}", suffix, 1)
            }
            _ => String::new(),
        }
    } else {
        let example = match provider {
            ProviderId::Custom => "https://opencode.ai/zen/go/v1",
            _ => meta.default_base_url.unwrap_or(""),
        };
        if example.is_empty() {
            String::new()
        } else {
            labels.base_url_example.replace("{}", example)
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let input = text_input::input_paragraph(&display, Some(title), colors::BLUE);
    frame.render_widget(input, chunks[0]);

    if !hint.is_empty() {
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            chunks[1],
        );
    }
}

/// Endpoint step for the Custom provider: an arrow-key selector between the
/// two supported API endpoint paths.
pub(super) fn build_endpoint_selector(state: &AppState, labels: &SettingsLabels) -> Text<'static> {
    let options = [
        ("chat/completions", labels.endpoint_chat_desc),
        ("messages", labels.endpoint_messages_desc),
    ];
    let mut lines = vec![Line::from(labels.endpoint_prompt.to_string())];
    for (value, desc) in options {
        let selected = state.settings.input.trim() == value;
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(colors::GREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(
            format!("{marker}{value} — {desc}"),
            style,
        )));
    }
    Text::from(lines)
}

pub(super) fn build_provider_setup_footer(
    state: &AppState,
    common: &CommonLabels,
    width: usize,
) -> String {
    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => build_footer_wrapped(
            &[
                ("↑/↓", common.navigate),
                ("Enter", common.select),
                ("Esc", common.back),
                ("?", common.help),
            ],
            width,
        ),
        ProviderSetupStep::BaseUrl => {
            if state.settings.provider_setup_provider == ProviderId::Custom {
                build_footer_wrapped(&[("Enter", common.save), ("Esc", common.back)], width)
            } else {
                build_footer_wrapped(
                    &[
                        ("Enter", common.next),
                        ("Esc", common.back),
                        ("?", common.help),
                    ],
                    width,
                )
            }
        }
        ProviderSetupStep::Endpoint => {
            if state.settings.provider_setup_provider == ProviderId::Custom {
                build_footer_wrapped(
                    &[
                        ("↑/↓", common.select),
                        ("Enter", common.save),
                        ("Esc", common.back),
                    ],
                    width,
                )
            } else {
                build_footer_wrapped(
                    &[
                        ("Enter", common.next),
                        ("Esc", common.back),
                        ("?", common.help),
                    ],
                    width,
                )
            }
        }
        ProviderSetupStep::ApiKey => {
            build_footer_wrapped(&[("Enter", common.save), ("Esc", common.back)], width)
        }
        ProviderSetupStep::Model => {
            if state.settings.model_picker.loading {
                build_footer_wrapped(&[("Esc", common.back), ("?", common.help)], width)
            } else if state.settings.model_picker.error.is_some() {
                build_footer_wrapped(
                    &[
                        ("Enter", common.manual),
                        ("r", common.retry),
                        ("Esc", common.back),
                        ("?", common.help),
                    ],
                    width,
                )
            } else if state.settings.model_picker.manual {
                build_footer_wrapped(&[("Enter", common.save), ("Esc", common.back)], width)
            } else if state.settings.model_picker.models.is_empty() {
                build_footer_wrapped(
                    &[
                        ("Enter", common.enter_manually),
                        ("Esc", common.back),
                        ("?", common.help),
                    ],
                    width,
                )
            } else {
                build_footer_wrapped(
                    &[
                        ("↑/↓", common.navigate),
                        ("Enter", common.select),
                        ("Esc", common.back),
                        ("?", common.help),
                    ],
                    width,
                )
            }
        }
    }
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
                        let labels = get_settings_labels(native_language_code(Some(config)));
                        state.settings.error = Some(labels.err_base_url_custom.to_string());
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
            state.settings.error = None;
        }
        KeyCode::Backspace if state.settings.provider_setup_provider == ProviderId::Custom => {
            state.settings.input.pop();
            state.settings.error = None;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_endpoint_step(state: &mut AppState, code: KeyCode) -> Result<()> {
    let custom = state.settings.provider_setup_provider == ProviderId::Custom;
    match code {
        KeyCode::Esc => {
            go_back_provider_setup_step(state);
        }
        KeyCode::Enter => {
            if let Some(config) = state.config.as_mut() {
                let provider = state.settings.provider_setup_provider;
                if custom {
                    let value = state.settings.input.trim().to_string();
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
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') if custom => {
            state.settings.input = if state.settings.input.trim() == "messages" {
                "chat/completions".to_string()
            } else {
                "messages".to_string()
            };
            state.settings.error = None;
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
                    let labels = get_settings_labels(native_language_code(Some(config)));
                    let hint = meta
                        .env_key
                        .map(|name| labels.env_var_or_set.replace("{name}", name))
                        .unwrap_or_default();
                    state.settings.error =
                        Some(labels.err_api_key_required.replace("{hint}", &hint));
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
            state.settings.error = None;
        }
        KeyCode::Backspace => {
            state.settings.input.pop();
            state.settings.error = None;
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
            let labels = get_settings_labels(native_language_code(state.config.as_ref()));
            state.settings.model_picker.error = None;
            if state.settings.model_picker.models.is_empty() {
                state.settings.model_picker.error = Some(labels.err_no_models.to_string());
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
        let labels = get_settings_labels(native_language_code(state.config.as_ref()));
        return Err(AppError::Config(labels.err_model_required.to_string()));
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
