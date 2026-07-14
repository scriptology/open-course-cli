use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::sync::Arc;

use crate::app::{AppState, LlmResult, View};
use crate::config::profile::UserProfile;
use crate::config::provider::{ProviderConfig, ProviderId};
use crate::config::{OpenCourseConfig, write_config};
use crate::core::language::{is_valid_language_code, normalize_language_code};
use crate::error::{AppError, Result};
use crate::llm::model_listing::{ModelInfo, list_models};
use crate::llm::provider::ProviderMeta;
use crate::ui::views::model_check;
use crate::ui::widgets::Logo;

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
    fn label(&self) -> &'static str {
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

    fn all() -> &'static [Step] {
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

const CEFR_LEVELS: &[&str] = &["A1", "A2", "B1", "B2", "C1", "C2"];
const BATCH_SIZES: &[&str] = &["2", "3", "4", "5"];

fn base_url_default(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Custom => "https://opencode.ai/zen/go/v1",
        ProviderId::Ollama => "http://localhost:11434/v1",
        _ => "",
    }
}

fn shows_base_url_step(provider: ProviderId) -> bool {
    matches!(provider, ProviderId::Custom | ProviderId::Ollama)
}

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
    pub model_picker_loading: bool,
    pub model_picker_error: Option<String>,
    pub model_picker_models: Vec<ModelInfo>,
    pub model_picker_selected: usize,
    pub model_picker_manual: bool,
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
            model_picker_loading: false,
            model_picker_error: None,
            model_picker_models: Vec::new(),
            model_picker_selected: 0,
            model_picker_manual: false,
        }
    }

    fn current_step(&self) -> Step {
        self.steps[self.active]
    }

    fn set_provider(&mut self, provider: ProviderId) {
        self.provider = provider;
        self.provider_index = ProviderId::all()
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(ProviderId::all().len().saturating_sub(1));
        if self.base_url.is_empty() || self.base_url == base_url_default(self.provider) {
            self.base_url = base_url_default(provider).to_string();
        }
    }

    fn load_input(&mut self) {
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

    fn apply_input(&mut self) -> Result<()> {
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
                            )))
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

fn display_input(input: &str, step: Step) -> String {
    if step == Step::ApiKey {
        "*".repeat(input.len())
    } else {
        input.to_string()
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let step = state.onboarding.current_step();
    let footer_text = match step {
        Step::Provider => "↑/↓: select provider | Enter: next | Esc: quit",
        Step::Cefr => "↑/↓: select level | Enter: next | Esc: quit",
        Step::BatchSize => "↑/↓: select batch size | Enter: next | Esc: quit",
        Step::Model if state.onboarding.model_picker_loading => "Loading models... | Esc: quit",
        Step::Model if state.onboarding.model_picker_error.is_some() => {
            "r: retry | m: manual | Esc: quit"
        }
        Step::Model if state.onboarding.model_picker_manual => {
            "Type model ID | Enter: next | Esc: quit"
        }
        Step::Model if !state.onboarding.model_picker_models.is_empty() => {
            "↑/↓: select model | Enter: next | Esc: quit"
        }
        Step::BaseUrl if !shows_base_url_step(state.onboarding.provider) => {
            "Enter: next | Esc: quit"
        }
        _ => "Tab/Enter: next | Shift+Tab: prev | Esc: quit",
    };
    let mut footer_lines = vec![Line::from(footer_text)];
    if !state.onboarding.error.is_empty() {
        footer_lines.push(
            Line::from(state.onboarding.error.clone()).style(Style::default().fg(Color::Red)),
        );
    }
    let footer_height = footer_lines.len() as u16;

    let accent = Color::Rgb(0, 122, 255);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // Header: logo + subtitle + global hint.
    let header_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Length(2)])
        .split(chunks[0]);

    frame.render_widget(Logo, header_chunks[0]);

    let subtitle = Text::from(vec![
        Line::from(Span::styled(
            "Set up your language learning profile",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Tab/Enter: continue | Shift+Tab: prev | Esc: quit | ↑/↓: select lists",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(Paragraph::new(subtitle), header_chunks[1]);

    // Step card with border.
    let progress = format!(
        "Step {} of {}",
        state.onboarding.active + 1,
        state.onboarding.steps.len()
    );
    let title = format!("{} — {}", step.label(), progress);
    let card_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            title,
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ));
    let card_inner = card_block.inner(chunks[1]);
    frame.render_widget(card_block, chunks[1]);

    if is_text_step(step) {
        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(card_inner);

        let input_paragraph = render_input_paragraph(&state.onboarding.input, step, accent);
        frame.render_widget(input_paragraph, inner_chunks[0]);

        let help_text = step_help_text(step, state);
        frame.render_widget(
            Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray)),
            inner_chunks[1],
        );
    } else {
        match step {
            Step::Provider | Step::Cefr | Step::BatchSize => {
                let help_text = step_help_text(step, state);
                frame.render_widget(
                    Paragraph::new(help_text).style(Style::default().fg(Color::White)),
                    card_inner,
                );
            }
            Step::Model => render_model_step(frame, card_inner, state),
            _ => {}
        }
    }

    frame.render_widget(
        Paragraph::new(footer_lines).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn is_text_step(step: Step) -> bool {
    matches!(
        step,
        Step::NativeLanguage
            | Step::TargetLanguage
            | Step::Age
            | Step::ApiKey
            | Step::BaseUrl
    )
}

fn render_input_paragraph(input: &str, step: Step, accent: Color) -> Paragraph<'_> {
    let display = display_input(input, step);
    let text = Text::from(Line::from(vec![
        Span::raw(display),
        Span::styled(
            "█",
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(accent)))
        .style(Style::default().fg(Color::White))
}

fn step_help_text(step: Step, state: &AppState) -> String {
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
    if meta.requires_api_key && !meta.api_key_optional {
        format!(
            "Enter API key for {}\n(required)",
            state.onboarding.provider.label()
        )
    } else {
        format!(
            "Enter API key for {}\n(optional — press Enter to skip)",
            state.onboarding.provider.label()
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

fn render_model_step(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
) {
    if state.onboarding.model_picker_loading {
        frame.render_widget(
            Paragraph::new("Fetching available models from provider...")
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    if let Some(err) = &state.onboarding.model_picker_error {
        frame.render_widget(
            Paragraph::new(format!(
                "Failed to load models:\n{}\n\nr: retry | m: enter manually",
                err
            ))
            .style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }

    if state.onboarding.model_picker_manual {
        frame.render_widget(
            Paragraph::new(
                "Enter model ID manually\n(e.g. gpt-4o-mini, claude-3-5-sonnet-20241022)",
            )
            .style(Style::default().fg(Color::White)),
            area,
        );
        return;
    }

    if state.onboarding.model_picker_models.is_empty() {
        frame.render_widget(
            Paragraph::new("No models found.\nr: retry | m: enter manually")
                .style(Style::default().fg(Color::White)),
            area,
        );
        return;
    }

    let model_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let items: Vec<ListItem> = state
        .onboarding
        .model_picker_models
        .iter()
        .map(|m| {
            let label = m
                .label
                .as_ref()
                .map(|l| format!("{} — {}", m.id, l))
                .unwrap_or_else(|| m.id.clone());
            ListItem::new(label)
        })
        .collect();

    let list = List::new(items).highlight_symbol("> ").highlight_style(
        Style::default()
            .fg(Color::Rgb(0, 122, 255))
            .add_modifier(Modifier::BOLD),
    );

    let mut list_state = ListState::default();
    list_state.select(Some(state.onboarding.model_picker_selected));
    frame.render_stateful_widget(list, model_chunks[0], &mut list_state);

    let selected = state.onboarding.model_picker_selected;
    let total = state.onboarding.model_picker_models.len();
    let info = format!(
        "Model {} of {} (m: manual, r: retry, Esc: quit)",
        selected + 1,
        total
    );
    frame.render_widget(
        Paragraph::new(info).style(Style::default().fg(Color::DarkGray)),
        model_chunks[1],
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let step = state.onboarding.current_step();
    match step {
        Step::Provider => handle_provider_key(state, code).await,
        Step::ApiKey => handle_text_key(state, code).await,
        Step::BaseUrl => handle_base_url_key(state, code).await,
        Step::Model => handle_model_key(state, code).await,
        Step::Cefr => handle_cefr_key(state, code).await,
        Step::BatchSize => handle_batch_size_key(state, code).await,
        _ => handle_text_key(state, code).await,
    }
}

fn handle_esc(state: &mut AppState) {
    match state.onboarding.mode {
        OnboardingMode::Initial => state.view = View::Quitting,
        OnboardingMode::AddPair => state.view = View::Pairs,
    }
}

async fn handle_provider_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let all = ProviderId::all();
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter | KeyCode::Char('\t') | KeyCode::Tab => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let idx = state.onboarding.provider_index;
            let new_idx = if idx == 0 { all.len() - 1 } else { idx - 1 };
            let new_provider = all[new_idx];
            state.onboarding.set_provider(new_provider);
            state.onboarding.input = new_provider.as_str().to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let idx = state.onboarding.provider_index;
            let new_idx = (idx + 1) % all.len();
            let new_provider = all[new_idx];
            state.onboarding.set_provider(new_provider);
            state.onboarding.input = new_provider.as_str().to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Char(c) => {
            let value = (state.onboarding.input.clone() + &c.to_string()).to_lowercase();
            if let Some((idx, p)) = all
                .iter()
                .enumerate()
                .find(|(_, p)| p.as_str().starts_with(&value))
            {
                state.onboarding.provider_index = idx;
                state.onboarding.set_provider(*p);
                state.onboarding.input = value;
            } else {
                state.onboarding.input = value;
            }
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
            let value = state.onboarding.input.to_lowercase();
            if let Some((idx, p)) = all
                .iter()
                .enumerate()
                .find(|(_, p)| p.as_str().starts_with(&value))
            {
                state.onboarding.provider_index = idx;
                state.onboarding.set_provider(*p);
            }
            state.onboarding.error.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_base_url_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Char('\t') | KeyCode::Tab | KeyCode::Enter => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab | KeyCode::Up if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Char(c) => {
            state.onboarding.input.push(c);
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_model_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.onboarding.model_picker_manual {
        match code {
            KeyCode::Esc => {
                state.onboarding.model_picker_manual = false;
                state.onboarding.load_input();
            }
            KeyCode::Char('\t') | KeyCode::Tab | KeyCode::Enter => {
                advance_onboarding(state).await?;
            }
            KeyCode::Char(c) => {
                state.onboarding.input.push(c);
                state.onboarding.error.clear();
            }
            KeyCode::Backspace => {
                state.onboarding.input.pop();
            }
            _ => {}
        }
        return Ok(());
    }

    if state.onboarding.model_picker_error.is_some() {
        match code {
            KeyCode::Esc => handle_esc(state),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                spawn_model_fetch(state);
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                state.onboarding.model_picker_manual = true;
                state.onboarding.load_input();
            }
            _ => {}
        }
        return Ok(());
    }

    let len = state.onboarding.model_picker_models.len();
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter => {
            let selected = state.onboarding.model_picker_selected;
            if let Some(model) = state.onboarding.model_picker_models.get(selected) {
                state.onboarding.model = model.id.clone();
                state.onboarding.input = model.id.clone();
            }
            advance_onboarding(state).await?;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let selected = state.onboarding.model_picker_selected;
            state.onboarding.model_picker_selected =
                if selected == 0 { len - 1 } else { selected - 1 };
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let selected = state.onboarding.model_picker_selected;
            state.onboarding.model_picker_selected = (selected + 1) % len;
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            state.onboarding.model_picker_manual = true;
            state.onboarding.input.clear();
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            spawn_model_fetch(state);
        }
        _ => {}
    }
    Ok(())
}

async fn handle_cefr_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter | KeyCode::Char('\t') | KeyCode::Tab => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let all = CEFR_LEVELS;
            let idx = all
                .iter()
                .position(|l| *l == state.onboarding.cefr)
                .unwrap_or(all.len() - 1);
            let new_idx = if idx == 0 { all.len() - 1 } else { idx - 1 };
            state.onboarding.cefr = all[new_idx].to_string();
            state.onboarding.input = state.onboarding.cefr.clone();
            state.onboarding.error.clear();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let all = CEFR_LEVELS;
            let idx = all
                .iter()
                .position(|l| *l == state.onboarding.cefr)
                .unwrap_or(all.len() - 1);
            let new_idx = (idx + 1) % all.len();
            state.onboarding.cefr = all[new_idx].to_string();
            state.onboarding.input = state.onboarding.cefr.clone();
            state.onboarding.error.clear();
        }
        KeyCode::Char(c) => {
            let value = (state.onboarding.input.clone() + &c.to_string()).to_uppercase();
            state.onboarding.input = value;
            if CEFR_LEVELS.contains(&state.onboarding.input.as_str()) {
                state.onboarding.cefr = state.onboarding.input.clone();
            }
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
            state.onboarding.input = state.onboarding.input.to_uppercase();
            if CEFR_LEVELS.contains(&state.onboarding.input.as_str()) {
                state.onboarding.cefr = state.onboarding.input.clone();
            }
            state.onboarding.error.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_batch_size_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter | KeyCode::Char('\t') | KeyCode::Tab => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let all = BATCH_SIZES;
            let current = state.onboarding.batch_size.to_string();
            let idx = all
                .iter()
                .position(|l| **l == current)
                .unwrap_or(all.len() - 1);
            let new_idx = if idx == 0 { all.len() - 1 } else { idx - 1 };
            state.onboarding.batch_size = all[new_idx].parse().unwrap_or(3);
            state.onboarding.input = state.onboarding.batch_size.to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let all = BATCH_SIZES;
            let current = state.onboarding.batch_size.to_string();
            let idx = all
                .iter()
                .position(|l| **l == current)
                .unwrap_or(all.len() - 1);
            let new_idx = (idx + 1) % all.len();
            state.onboarding.batch_size = all[new_idx].parse().unwrap_or(3);
            state.onboarding.input = state.onboarding.batch_size.to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let value = (state.onboarding.input.clone() + &c.to_string()).to_string();
            state.onboarding.input = value.clone();
            if let Ok(n) = value.parse::<u32>()
                && (2..=5).contains(&n)
            {
                state.onboarding.batch_size = n;
            }
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
            if let Ok(n) = state.onboarding.input.parse::<u32>()
                && (2..=5).contains(&n)
            {
                state.onboarding.batch_size = n;
            }
            state.onboarding.error.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_text_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Char('\t') | KeyCode::Tab | KeyCode::Enter | KeyCode::Down => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab | KeyCode::Up if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Char(c) => {
            state.onboarding.input.push(c);
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn advance_onboarding(state: &mut AppState) -> Result<()> {
    if let Err(e) = state.onboarding.apply_input() {
        state.onboarding.error = e.to_string();
        return Ok(());
    }
    if state.onboarding.current_step() == Step::Model {
        let config = build_config_from_onboarding(&state.onboarding);
        model_check::start(state, config, View::Onboarding);
        return Ok(());
    }
    if state.onboarding.active == state.onboarding.steps.len() - 1 {
        finish_onboarding(state).await?;
    } else {
        state.onboarding.active += 1;
        state.onboarding.load_input();
        if state.onboarding.current_step() == Step::Model {
            spawn_model_fetch(state);
        }
    }
    Ok(())
}

pub(crate) async fn finish_onboarding(state: &mut AppState) -> Result<()> {
    match state.onboarding.mode {
        OnboardingMode::Initial => {
            let config = build_config_from_onboarding(&state.onboarding);
            let pair_id = config.active_pair.clone();
            write_config(&config, &state.data_dir)?;
            state.config = Some(config);

            let db_path = crate::config::pair_db_path(&state.data_dir, &pair_id);
            let db = crate::db::Database::connect(&db_path).await?;
            state.db = Arc::new(db);

            state.view = View::Curriculum;
        }
        OnboardingMode::AddPair => {
            let profile = build_profile_from_onboarding(&state.onboarding);
            let config = state.config.as_mut().ok_or_else(|| {
                AppError::Config("No config available".to_string())
            })?;
            let new_id = config.add_pair(profile)?.to_string();
            write_config(config, &state.data_dir)?;
            crate::app::switch_pair(state, &new_id).await?;
        }
    }
    Ok(())
}

fn build_profile_from_onboarding(onboarding: &OnboardingState) -> UserProfile {
    UserProfile {
        native_language: onboarding.native_language.clone(),
        target_language: onboarding.target_language.clone(),
        age: if onboarding.age.is_empty() {
            None
        } else {
            onboarding.age.parse().ok()
        },
        self_assessed_cefr: Some(onboarding.cefr.clone()),
    }
}

fn build_config_from_onboarding(onboarding: &OnboardingState) -> OpenCourseConfig {
    let profile = build_profile_from_onboarding(onboarding);

    let provider_config = ProviderConfig::ApiKey {
        api_key: if onboarding.api_key.is_empty() {
            None
        } else {
            Some(onboarding.api_key.clone())
        },
        model: onboarding.model.clone(),
        base_url: if onboarding.base_url.is_empty() {
            None
        } else {
            Some(onboarding.base_url.clone())
        },
        endpoint: None,
        reasoning_effort: None,
    };

    let mut config = OpenCourseConfig::new(onboarding.provider, provider_config, profile);
    config.preferences.batch_size = onboarding.batch_size;
    config
}

fn spawn_model_fetch(state: &mut AppState) {
    let provider_id = state.onboarding.provider;
    let api_key = state.onboarding.api_key.clone();
    let base_url = if state.onboarding.base_url.is_empty() {
        ProviderMeta::for_provider(provider_id)
            .default_base_url
            .map(|s| s.to_string())
    } else {
        Some(state.onboarding.base_url.clone())
    };

    state.onboarding.model_picker_loading = true;
    state.onboarding.model_picker_error = None;
    state.onboarding.model_picker_manual = false;
    state.onboarding.model_picker_models.clear();
    state.onboarding.model_picker_selected = 0;

    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let api_key_ref = if api_key.is_empty() {
            None
        } else {
            Some(api_key.as_str())
        };
        let result = list_models(provider_id, api_key_ref, base_url.as_deref()).await;
        let _ = tx.send(LlmResult::OnboardingModels(result)).await;
    });
}
