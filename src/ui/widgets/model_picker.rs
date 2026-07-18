use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use tokio::sync::mpsc;

use crate::app::LlmResult;
use crate::config::provider::ProviderId;
use crate::error::Result;
use crate::llm::model_listing::{ModelInfo, list_models};
use crate::ui::colors;

/// Shared "pick a model from the provider" state machine, used by the
/// settings provider wizard and by onboarding.
#[derive(Debug, Clone, Default)]
pub struct ModelPickerState {
    pub loading: bool,
    pub error: Option<String>,
    pub models: Vec<ModelInfo>,
    pub selected: usize,
    pub manual: bool,
}

impl ModelPickerState {
    /// Clears the fetched list and selection, keeping the loading flag.
    pub fn reset(&mut self) {
        self.models.clear();
        self.selected = 0;
        self.error = None;
        self.manual = false;
    }

    /// Applies a model-list result received through the LLM channel.
    pub fn apply_result(&mut self, result: Result<Vec<ModelInfo>>) {
        self.loading = false;
        match result {
            Ok(models) => {
                self.models = models;
                self.selected = 0;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    /// Whether the fetched model list should be rendered.
    pub fn shows_list(&self) -> bool {
        !self.loading && self.error.is_none() && !self.manual && !self.models.is_empty()
    }
}

/// What the hosting view should do after [`handle_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPickerAction {
    /// The key had no view-visible effect (includes internal list navigation).
    Ignored,
    /// Esc outside manual mode: leave the model step.
    Back,
    /// `r` pressed: the view should respawn the model load.
    Retry,
    /// Enter on a non-empty list; carries the selected model id.
    Select(String),
    /// Enter (or Tab, where enabled) in manual mode: confirm the typed input.
    ConfirmManual,
    /// Enter with an empty list.
    EmptyEnter,
    /// Entered manual mode from the error state; the view may preload the input.
    EnterManual,
    /// Esc in manual mode; the view may reload the input.
    ExitManual,
    /// A character was appended to the input.
    InputPushed,
    /// A character was removed from the input.
    InputPopped,
}

/// Tailors the state machine to the hosting view.
#[derive(Debug, Clone, Copy)]
pub struct ModelPickerOptions {
    /// While loading, swallow all keys except Esc (settings) instead of
    /// falling through to the list state (onboarding).
    pub gate_keys_while_loading: bool,
    /// Enter (settings), not `m` (onboarding), enters manual mode from the
    /// error state.
    pub error_enter_manual: bool,
    /// `m`/`r` shortcuts work in the list state (onboarding).
    pub list_shortcuts: bool,
    /// Tab confirms manual input like Enter (onboarding).
    pub manual_tab_confirms: bool,
}

impl ModelPickerOptions {
    pub const SETTINGS: Self = Self {
        gate_keys_while_loading: true,
        error_enter_manual: true,
        list_shortcuts: false,
        manual_tab_confirms: false,
    };

    pub const ONBOARDING: Self = Self {
        gate_keys_while_loading: false,
        error_enter_manual: false,
        list_shortcuts: true,
        manual_tab_confirms: true,
    };
}

/// Handles a key press for the picker, mutating only the picker state and the
/// text input. View-specific follow-ups are reported through the returned
/// [`ModelPickerAction`].
pub fn handle_key(
    picker: &mut ModelPickerState,
    input: &mut String,
    code: KeyCode,
    options: &ModelPickerOptions,
) -> ModelPickerAction {
    if options.gate_keys_while_loading && picker.loading {
        return if code == KeyCode::Esc {
            ModelPickerAction::Back
        } else {
            ModelPickerAction::Ignored
        };
    }

    if picker.manual {
        return match code {
            KeyCode::Esc => {
                picker.manual = false;
                ModelPickerAction::ExitManual
            }
            KeyCode::Enter => ModelPickerAction::ConfirmManual,
            KeyCode::Char('\t') | KeyCode::Tab if options.manual_tab_confirms => {
                ModelPickerAction::ConfirmManual
            }
            KeyCode::Char(c) => {
                input.push(c);
                ModelPickerAction::InputPushed
            }
            KeyCode::Backspace => {
                input.pop();
                ModelPickerAction::InputPopped
            }
            _ => ModelPickerAction::Ignored,
        };
    }

    if picker.error.is_some() {
        return match code {
            KeyCode::Esc => ModelPickerAction::Back,
            KeyCode::Enter if options.error_enter_manual => {
                picker.manual = true;
                ModelPickerAction::EnterManual
            }
            KeyCode::Char('m') | KeyCode::Char('M') if !options.error_enter_manual => {
                picker.manual = true;
                ModelPickerAction::EnterManual
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                picker.error = None;
                picker.manual = false;
                ModelPickerAction::Retry
            }
            _ => ModelPickerAction::Ignored,
        };
    }

    let len = picker.models.len();
    match code {
        KeyCode::Esc => ModelPickerAction::Back,
        KeyCode::Enter => {
            if len == 0 {
                ModelPickerAction::EmptyEnter
            } else if let Some(model) = picker.models.get(picker.selected) {
                ModelPickerAction::Select(model.id.clone())
            } else {
                ModelPickerAction::Ignored
            }
        }
        KeyCode::Up | KeyCode::Char('k') if len > 0 => {
            picker.selected = (picker.selected + len - 1) % len;
            ModelPickerAction::Ignored
        }
        KeyCode::Down | KeyCode::Char('j') if len > 0 => {
            picker.selected = (picker.selected + 1) % len;
            ModelPickerAction::Ignored
        }
        KeyCode::Char('m') | KeyCode::Char('M') if options.list_shortcuts => {
            picker.manual = true;
            input.clear();
            ModelPickerAction::Ignored
        }
        KeyCode::Char('r') | KeyCode::Char('R') if options.list_shortcuts => {
            ModelPickerAction::Retry
        }
        _ => ModelPickerAction::Ignored,
    }
}

/// Starts an async model fetch, reporting through the LLM channel with the
/// variant produced by `map_result`. Sets the loading flags; clearing the
/// previous list is the caller's choice (`reset`).
pub fn spawn_load(
    picker: &mut ModelPickerState,
    tx: mpsc::Sender<LlmResult>,
    provider: ProviderId,
    api_key: Option<String>,
    base_url: Option<String>,
    map_result: fn(Result<Vec<ModelInfo>>) -> LlmResult,
) {
    picker.loading = true;
    picker.error = None;
    picker.manual = false;
    tokio::spawn(async move {
        let result = list_models(provider, api_key.as_deref(), base_url.as_deref()).await;
        let _ = tx.send(map_result(result)).await;
    });
}

/// Renders the fetched model list with a `> ` highlight and an optional info
/// line below it.
pub fn draw_model_list(
    frame: &mut ratatui::Frame,
    area: Rect,
    picker: &ModelPickerState,
    info: Option<&str>,
) {
    let (list_area, info_area) = if info.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let items: Vec<ListItem> = picker
        .models
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
            .fg(colors::BLUE)
            .add_modifier(Modifier::BOLD),
    );

    let mut list_state = ListState::default();
    list_state.select(Some(picker.selected));
    frame.render_stateful_widget(list, list_area, &mut list_state);

    if let (Some(info), Some(info_area)) = (info, info_area) {
        frame.render_widget(
            Paragraph::new(info).style(Style::default().fg(Color::DarkGray)),
            info_area,
        );
    }
}
