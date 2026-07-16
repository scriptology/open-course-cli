use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::{AppState, LlmResult, View};
use crate::config::OpenCourseConfig;
use crate::config::profile::HintMode;
use crate::config::provider::{ProviderConfig, ProviderId};
use crate::config::write_config;
use crate::error::{AppError, Result};
use crate::llm::model_listing::{ModelInfo, list_models};
use crate::llm::provider::ProviderMeta;
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};
use crate::ui::views::model_check;
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Provider,
    Profile,
    Session,
    Data,
}

impl Section {
    fn label(&self) -> &'static str {
        match self {
            Section::Provider => "Provider",
            Section::Profile => "Profile",
            Section::Session => "Session",
            Section::Data => "Data",
        }
    }

    fn all() -> &'static [Section] {
        &[
            Section::Provider,
            Section::Profile,
            Section::Session,
            Section::Data,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetAction {
    Progress,
    History,
    Curriculum,
    Reviews,
    All,
}

impl ResetAction {
    fn label(&self) -> &'static str {
        match self {
            ResetAction::Progress => "reset progress",
            ResetAction::History => "reset history",
            ResetAction::Curriculum => "reset curriculum",
            ResetAction::Reviews => "reset reviews",
            ResetAction::All => "reset all data",
        }
    }

    fn from_field(field: usize) -> Option<Self> {
        match field {
            0 => Some(ResetAction::Progress),
            1 => Some(ResetAction::History),
            2 => Some(ResetAction::Curriculum),
            3 => Some(ResetAction::Reviews),
            4 => Some(ResetAction::All),
            _ => None,
        }
    }
}

const CEFR_LEVELS: &[&str] = &["A1", "A2", "B1", "B2", "C1", "C2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderSetupStep {
    #[default]
    SelectProvider,
    BaseUrl,
    Endpoint,
    ApiKey,
    Model,
}

#[derive(Debug, Clone)]
pub struct SettingsState {
    pub section: Section,
    pub active_field: usize,
    pub input: String,
    pub error: Option<String>,
    pub pending_reset: Option<ResetAction>,
    pub in_section: bool,
    pub section_list_state: ListState,
    loaded_field: Option<(Section, usize)>,

    // Provider setup wizard state
    pub provider_setup_step: ProviderSetupStep,
    pub provider_setup_provider: ProviderId,
    pub provider_setup_models: Vec<ModelInfo>,
    pub provider_setup_model_selected: usize,
    pub provider_setup_loading: bool,
    pub provider_setup_error: Option<String>,
    pub provider_setup_manual_model: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsState {
    pub fn new() -> Self {
        let mut section_list_state = ListState::default();
        section_list_state.select(Some(0));
        Self {
            section: Section::default(),
            active_field: 0,
            input: String::new(),
            error: None,
            pending_reset: None,
            in_section: false,
            section_list_state,
            loaded_field: None,
            provider_setup_step: ProviderSetupStep::SelectProvider,
            provider_setup_provider: ProviderId::OpenAi,
            provider_setup_models: Vec::new(),
            provider_setup_model_selected: 0,
            provider_setup_loading: false,
            provider_setup_error: None,
            provider_setup_manual_model: false,
        }
    }

    fn field_count(&self) -> usize {
        match self.section {
            Section::Provider => 4,
            Section::Profile => 2,
            Section::Session => 2,
            Section::Data => 5,
        }
    }

    fn next_field(&mut self) {
        let count = self.field_count();
        self.active_field = (self.active_field + 1) % count;
    }

    fn prev_field(&mut self) {
        let count = self.field_count();
        self.active_field = (self.active_field + count - 1) % count;
    }

    fn is_text_field(&self) -> bool {
        match self.section {
            Section::Data => false,
            Section::Session if self.active_field == 1 => false,
            _ => true,
        }
    }

    pub fn reset_to_section_list(&mut self) {
        self.in_section = false;
        self.loaded_field = None;
    }

    fn ensure_input_loaded(&mut self, config: &OpenCourseConfig) {
        if self.section == Section::Provider {
            if self.loaded_field != Some((Section::Provider, 0)) {
                self.load_provider_setup_input(config);
                self.loaded_field = Some((Section::Provider, 0));
            }
            return;
        }
        if self.loaded_field != Some((self.section, self.active_field)) {
            self.load_input(config);
            self.loaded_field = Some((self.section, self.active_field));
        }
    }

    fn load_provider_setup_input(&mut self, config: &OpenCourseConfig) {
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

    fn load_input(&mut self, config: &OpenCourseConfig) {
        self.input = match self.section {
            Section::Profile => match self.active_field {
                0 => config
                    .active_profile()
                    .age
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                1 => config
                    .active_profile()
                    .self_assessed_cefr
                    .clone()
                    .unwrap_or_default(),
                _ => unreachable!(),
            },
            Section::Session => match self.active_field {
                0 => config.preferences.batch_size.to_string(),
                1 => match config.preferences.hint_mode {
                    HintMode::Auto => "auto".to_string(),
                    HintMode::OnDemand => "on-demand".to_string(),
                },
                _ => unreachable!(),
            },
            Section::Data => String::new(),
            Section::Provider => String::new(),
        };
    }

    fn apply_input(&mut self, config: &mut OpenCourseConfig) -> Result<()> {
        let value = self.input.trim().to_string();
        match self.section {
            Section::Provider => {}
            Section::Profile => match self.active_field {
                0 => {
                    config.active_profile_mut().age = if value.is_empty() {
                        None
                    } else {
                        match value.parse::<u32>() {
                            Ok(age) if (1..=120).contains(&age) => Some(age),
                            _ => {
                                return Err(AppError::Config(format!(
                                    "Age must be a number between 1 and 120: {value}"
                                )))
                            }
                        }
                    };
                }
                1 => {
                    if !value.is_empty() && !CEFR_LEVELS.contains(&value.to_uppercase().as_str()) {
                        return Err(AppError::Config(format!("Invalid CEFR level: {value}")));
                    }
                    config.active_profile_mut().self_assessed_cefr = if value.is_empty() {
                        None
                    } else {
                        Some(value.to_uppercase())
                    };
                }
                _ => unreachable!(),
            },
            Section::Session => match self.active_field {
                0 => {
                    let size = value
                        .parse::<u32>()
                        .map_err(|_| AppError::Config(format!("Invalid batch size: {value}")))?;
                    if !(2..=5).contains(&size) {
                        return Err(AppError::Config("Batch size must be 2-5".to_string()));
                    }
                    config.preferences.batch_size = size;
                }
                1 => {
                    config.preferences.hint_mode = match config.preferences.hint_mode {
                        HintMode::Auto => HintMode::OnDemand,
                        HintMode::OnDemand => HintMode::Auto,
                    };
                }
                _ => unreachable!(),
            },
            Section::Data => {}
        }
        Ok(())
    }

    fn save(&mut self, config: &mut OpenCourseConfig, data_dir: &std::path::Path) -> Result<()> {
        self.apply_input(config)?;
        write_config(config, data_dir)?;
        Ok(())
    }
}

fn field_label(section: Section, field: usize) -> &'static str {
    match section {
        Section::Provider => "",
        Section::Profile => match field {
            0 => "Age",
            1 => "CEFR",
            _ => unreachable!(),
        },
        Section::Session => match field {
            0 => "Batch size",
            1 => "Hint mode",
            _ => unreachable!(),
        },
        Section::Data => match field {
            0 => "Reset progress",
            1 => "Reset history",
            2 => "Reset curriculum",
            3 => "Reset reviews",
            4 => "Reset all",
            _ => unreachable!(),
        },
    }
}

fn field_value(config: &OpenCourseConfig, section: Section, field: usize) -> String {
    match section {
        Section::Provider => String::new(),
        Section::Profile => match field {
            0 => config
                .active_profile()
                .age
                .map(|a| a.to_string())
                .unwrap_or_else(|| "(none)".to_string()),
            1 => config
                .active_profile()
                .self_assessed_cefr
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
            _ => unreachable!(),
        },
        Section::Session => match field {
            0 => config.preferences.batch_size.to_string(),
            1 => match config.preferences.hint_mode {
                HintMode::Auto => "auto".to_string(),
                HintMode::OnDemand => "on-demand".to_string(),
            },
            _ => unreachable!(),
        },
        Section::Data => match field {
            0 => "Clear all progress scores".to_string(),
            1 => "Clear all session history".to_string(),
            2 => "Clear all curriculum topics".to_string(),
            3 => "Clear all topic reviews".to_string(),
            4 => "Clear all data".to_string(),
            _ => unreachable!(),
        },
    }
}

fn build_body(state: &AppState) -> String {
    let config = match state.config.as_ref() {
        Some(c) => c,
        None => return "No configuration available. Press Esc to return.".to_string(),
    };

    if let Some(action) = state.settings.pending_reset {
        return format!(
            "Confirm {}\n\nPress y to confirm, any other key to cancel.",
            action.label()
        );
    }

    if state.settings.section == Section::Provider && state.settings.in_section {
        return build_provider_setup_body(state, config);
    }

    let mut lines = vec![String::new()];

    let count = state.settings.field_count();
    for i in 0..count {
        let is_active = i == state.settings.active_field;
        let marker = if is_active { "> " } else { "  " };
        let label = field_label(state.settings.section, i);
        let value = if is_active && state.settings.section != Section::Data {
            state.settings.input.clone()
        } else {
            field_value(config, state.settings.section, i)
        };
        lines.push(format!("{}{}: {}", marker, label, value));
    }

    lines.join("\n")
}

fn build_provider_setup_body(state: &AppState, config: &OpenCourseConfig) -> String {
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
            format!("API key: {}", masked)
        }
        ProviderSetupStep::Model => {
            if state.settings.provider_setup_loading {
                "Loading models...".to_string()
            } else if let Some(err) = &state.settings.provider_setup_error {
                format!(
                    "Error loading models: {}\n\nEnter: enter manually | r: retry | Esc: back",
                    err
                )
            } else if state.settings.provider_setup_manual_model {
                format!("Model (manual): {}", state.settings.input)
            } else if state.settings.provider_setup_models.is_empty() {
                "No models loaded.\nEnter: enter manually".to_string()
            } else {
                let mut lines = vec!["Select model:".to_string()];
                for (i, m) in state.settings.provider_setup_models.iter().enumerate() {
                    let marker = if i == state.settings.provider_setup_model_selected {
                        "> "
                    } else {
                        "  "
                    };
                    lines.push(format!("{}{}", marker, m.id));
                }
                lines.join("\n")
            }
        }
    }
}

fn build_footer(state: &AppState) -> String {
    if let Some(action) = state.settings.pending_reset {
        return format!("y: confirm {} | any other key: cancel", action.label());
    }

    if state.settings.section == Section::Provider && state.settings.in_section {
        return build_provider_setup_footer(state);
    }

    let mut lines = vec![String::new()];
    if state.settings.section == Section::Data {
        lines[0] = "Tab/Shift+Tab: action | Enter: reset | Esc: back".to_string();
    } else {
        lines[0] = "Tab/Shift+Tab: field | Enter: save | Esc: back".to_string();
    }

    if let Some(err) = &state.settings.error {
        lines.push(err.clone());
    }

    lines.join("\n")
}

fn build_provider_setup_footer(state: &AppState) -> String {
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
            if state.settings.provider_setup_loading {
                "Esc: back"
            } else if state.settings.provider_setup_error.is_some() {
                "Enter: manual | r: retry | Esc: back"
            } else if state.settings.provider_setup_manual_model {
                "Enter: save | Esc: back"
            } else if state.settings.provider_setup_models.is_empty() {
                "Enter: enter manually | Esc: back"
            } else {
                "↑/↓: navigate | Enter: select | Esc: back"
            }
        }
    }
    .to_string()
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    if let Some(config) = state.config.as_ref() {
        state.settings.ensure_input_loaded(config);
    }

    if !state.settings.in_section {
        draw_section_picker(frame, area, state);
    } else {
        draw_section_page(frame, area, state);
    }
}

fn draw_section_picker(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
) {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                labels.settings,
                Style::default()
                    .fg(colors::BLUE)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ])),
        chunks[0],
    );

    let items: Vec<ListItem> = Section::all()
        .iter()
        .map(|s| ListItem::new(s.label()))
        .collect();

    let list = List::new(items).highlight_symbol("> ").highlight_style(
        Style::default()
            .fg(colors::BLUE)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(list, chunks[1], &mut state.settings.section_list_state);

    frame.render_widget(
        Paragraph::new("↑/↓: navigate | Enter: open | Esc: back")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn draw_section_page(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
) {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let footer_text = build_footer(state);
    let footer_height = footer_text.lines().count() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    let header = Text::from(vec![
        Line::from(Span::styled(
            labels.settings,
            Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            state.settings.section.label(),
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(Paragraph::new(header), chunks[0]);

    frame.render_widget(
        Paragraph::new(build_body(state)).style(Style::default().fg(Color::White)),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.config.is_none() {
        if code == KeyCode::Esc {
            state.view = View::Dashboard;
        }
        return Ok(());
    }

    if let Some(action) = state.settings.pending_reset {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                execute_reset(state, action).await?;
                state.settings.pending_reset = None;
            }
            _ => {
                state.settings.pending_reset = None;
            }
        }
        return Ok(());
    }

    if !state.settings.in_section {
        let sections = Section::all();
        match code {
            KeyCode::Esc => state.view = View::Dashboard,
            KeyCode::Char('j') | KeyCode::Down => {
                select_next_wrapping(&mut state.settings.section_list_state, sections.len());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                select_previous_wrapping(&mut state.settings.section_list_state, sections.len());
            }
            KeyCode::Enter => {
                let selected = state.settings.section_list_state.selected().unwrap_or(0);
                let section = sections[selected];
                state.settings.section = section;
                state.settings.active_field = 0;
                state.settings.loaded_field = None;
                state.settings.error = None;
                state.settings.in_section = true;
                if section == Section::Provider {
                    init_provider_setup(state);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    if state.settings.section == Section::Provider {
        return handle_provider_setup_key(state, code).await;
    }

    match code {
        KeyCode::Esc => {
            state.settings.in_section = false;
            state.settings.error = None;
        }
        KeyCode::Char('\t') | KeyCode::Tab => {
            state.settings.next_field();
            state.settings.loaded_field = None;
            state.settings.error = None;
        }
        KeyCode::BackTab => {
            state.settings.prev_field();
            state.settings.loaded_field = None;
            state.settings.error = None;
        }
        KeyCode::Enter => {
            if state.settings.section == Section::Data {
                if let Some(action) = ResetAction::from_field(state.settings.active_field) {
                    state.settings.pending_reset = Some(action);
                }
            } else if let Some(config) = state.config.as_mut() {
                if let Err(e) = state.settings.save(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                } else {
                    state.settings.error = None;
                }
            }
        }
        KeyCode::Char(c) if state.settings.is_text_field() => {
            state.settings.input.push(c);
        }
        KeyCode::Backspace if state.settings.is_text_field() => {
            state.settings.input.pop();
        }
        _ => {}
    }

    Ok(())
}

pub fn spawn_provider_model_load(state: &mut AppState) {
    let Some(config) = state.config.clone() else {
        return;
    };
    let provider = state.settings.provider_setup_provider;
    let provider_config = match config.providers.get(&provider) {
        Some(p) => p.clone(),
        None => return,
    };

    state.settings.provider_setup_loading = true;
    state.settings.provider_setup_error = None;
    state.settings.provider_setup_manual_model = false;

    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let result = list_models(
            provider,
            provider_config.api_key(),
            provider_config.base_url(),
        )
        .await;
        let _ = tx.send(LlmResult::Models(result)).await;
    });
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
    state.settings.provider_setup_models.clear();
    state.settings.provider_setup_model_selected = 0;
    state.settings.provider_setup_error = None;
    state.settings.provider_setup_manual_model = false;
    state.settings.loaded_field = None;
    state.settings.input = config
        .providers
        .get(&config.active_provider)
        .map(|p| p.model().to_string())
        .unwrap_or_default();
    spawn_provider_model_load(state);
}

fn init_provider_setup(state: &mut AppState) {
    let Some(config) = state.config.as_ref() else {
        return;
    };
    let provider = config.active_provider;
    state.settings.provider_setup_step = ProviderSetupStep::SelectProvider;
    state.settings.provider_setup_provider = provider;
    state.settings.provider_setup_models.clear();
    state.settings.provider_setup_model_selected = 0;
    state.settings.provider_setup_loading = false;
    state.settings.provider_setup_error = None;
    state.settings.provider_setup_manual_model = false;
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

fn advance_provider_setup_step(state: &mut AppState) {
    let provider = state.settings.provider_setup_provider;
    let next = match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => ProviderSetupStep::BaseUrl,
        ProviderSetupStep::BaseUrl => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else {
                let meta = ProviderMeta::for_provider(provider);
                if meta.requires_api_key {
                    ProviderSetupStep::ApiKey
                } else {
                    ProviderSetupStep::Model
                }
            }
        }
        ProviderSetupStep::Endpoint => {
            let meta = ProviderMeta::for_provider(provider);
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
        && state.settings.provider_setup_models.is_empty()
        && !state.settings.provider_setup_loading
    {
        spawn_provider_model_load(state);
    }
}

fn go_back_provider_setup_step(state: &mut AppState) {
    let provider = state.settings.provider_setup_provider;
    let prev = match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => {
            state.settings.in_section = false;
            return;
        }
        ProviderSetupStep::BaseUrl => ProviderSetupStep::SelectProvider,
        ProviderSetupStep::Endpoint => ProviderSetupStep::BaseUrl,
        ProviderSetupStep::ApiKey => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else {
                ProviderSetupStep::BaseUrl
            }
        }
        ProviderSetupStep::Model => {
            if provider == ProviderId::Custom {
                ProviderSetupStep::Endpoint
            } else {
                ProviderSetupStep::ApiKey
            }
        }
    };
    state.settings.provider_setup_step = prev;
    state.settings.loaded_field = None;
    if let Some(config) = state.config.as_ref() {
        state.settings.load_provider_setup_input(config);
    }
}

async fn handle_provider_setup_key(state: &mut AppState, code: KeyCode) -> Result<()> {
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
                state.settings.provider_setup_models.clear();
                state.settings.provider_setup_model_selected = 0;
                state.settings.provider_setup_error = None;
                state.settings.provider_setup_manual_model = false;
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
                        let updated = ProviderConfig::ApiKey {
                            api_key: provider_config.api_key().map(|s| s.to_string()),
                            model: provider_config.model().to_string(),
                            base_url: Some(value),
                            endpoint: Some(provider_config.endpoint().to_string()),
                            reasoning_effort: provider_config
                                .reasoning_effort()
                                .map(|s| s.to_string()),
                        };
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
                        let updated = ProviderConfig::ApiKey {
                            api_key: provider_config.api_key().map(|s| s.to_string()),
                            model: provider_config.model().to_string(),
                            base_url: provider_config.base_url().map(|s| s.to_string()),
                            endpoint: Some(value),
                            reasoning_effort: provider_config
                                .reasoning_effort()
                                .map(|s| s.to_string()),
                        };
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
                if meta.requires_api_key && !meta.api_key_optional && value.is_empty() {
                    state.settings.error =
                        Some("API key is required for this provider".to_string());
                    return Ok(());
                }
                if let Some(provider_config) = config.providers.get(&provider) {
                    let updated = ProviderConfig::ApiKey {
                        api_key: if value.is_empty() { None } else { Some(value) },
                        model: provider_config.model().to_string(),
                        base_url: provider_config.base_url().map(|s| s.to_string()),
                        endpoint: Some(provider_config.endpoint().to_string()),
                        reasoning_effort: provider_config.reasoning_effort().map(|s| s.to_string()),
                    };
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
    if state.settings.provider_setup_loading {
        if code == KeyCode::Esc {
            go_back_provider_setup_step(state);
        }
        return Ok(());
    }

    if state.settings.provider_setup_error.is_some() {
        match code {
            KeyCode::Esc => {
                go_back_provider_setup_step(state);
            }
            KeyCode::Enter => {
                state.settings.provider_setup_error = None;
                state.settings.provider_setup_manual_model = true;
                if let Some(config) = state.config.as_ref() {
                    state.settings.load_provider_setup_input(config);
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                state.settings.provider_setup_error = None;
                state.settings.provider_setup_manual_model = false;
                spawn_provider_model_load(state);
            }
            _ => {}
        }
        return Ok(());
    }

    if state.settings.provider_setup_manual_model {
        match code {
            KeyCode::Esc => {
                state.settings.provider_setup_manual_model = false;
                state.settings.provider_setup_error = None;
                if state.settings.provider_setup_models.is_empty() {
                    state.settings.provider_setup_error = Some("No models loaded".to_string());
                }
            }
            KeyCode::Enter => {
                let value = state.settings.input.trim().to_string();
                if let Err(e) = save_model_and_run_diagnostics(state, value) {
                    state.settings.error = Some(e.to_string());
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
        return Ok(());
    }

    let models = state.settings.provider_setup_models.clone();
    let selected = state.settings.provider_setup_model_selected;
    match code {
        KeyCode::Esc => {
            go_back_provider_setup_step(state);
        }
        KeyCode::Enter if models.is_empty() => {
            state.settings.provider_setup_manual_model = true;
            if let Some(config) = state.config.as_ref() {
                state.settings.load_provider_setup_input(config);
            }
        }
        KeyCode::Up | KeyCode::Char('k') if !models.is_empty() => {
            state.settings.provider_setup_model_selected =
                (selected + models.len() - 1) % models.len();
        }
        KeyCode::Down | KeyCode::Char('j') if !models.is_empty() => {
            state.settings.provider_setup_model_selected = (selected + 1) % models.len();
        }
        KeyCode::Enter => {
            if let Some(model) = models.get(selected).cloned()
                && let Err(e) = save_model_and_run_diagnostics(state, model.id)
            {
                state.settings.error = Some(e.to_string());
            }
        }
        _ => {}
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
        let updated = ProviderConfig::ApiKey {
            api_key: provider_config.api_key().map(|s| s.to_string()),
            model: model_id,
            base_url: provider_config.base_url().map(|s| s.to_string()),
            endpoint: Some(provider_config.endpoint().to_string()),
            reasoning_effort: provider_config.reasoning_effort().map(|s| s.to_string()),
        };
        config.providers.insert(provider, updated);
        write_config(config, &state.data_dir)?;
        config.clone()
    };
    state.settings.error = None;
    state.settings.in_section = true;
    state.settings.provider_setup_step = ProviderSetupStep::Model;
    state.settings.provider_setup_manual_model = false;
    state.settings.loaded_field = None;
    model_check::start(state, config_clone, View::Settings);
    Ok(())
}

async fn execute_reset(state: &mut AppState, action: ResetAction) -> Result<()> {
    let db = state.db.clone();
    match action {
        ResetAction::Progress => {
            db.progress().reset().await?;
        }
        ResetAction::History => {
            db.history().reset().await?;
        }
        ResetAction::Curriculum => {
            db.curriculum().reset().await?;
        }
        ResetAction::Reviews => {
            db.reviews().reset().await?;
        }
        ResetAction::All => {
            db.progress().reset().await?;
            db.history().reset().await?;
            db.curriculum().reset().await?;
            db.reviews().reset().await?;
        }
    }
    Ok(())
}
