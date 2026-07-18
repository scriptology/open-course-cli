mod data;
mod fields;
mod provider_setup;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::{AppState, View};
use crate::config::OpenCourseConfig;
use crate::config::provider::ProviderId;
use crate::error::Result;
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};
use crate::ui::widgets::{draw_confirmation, model_picker};

pub use data::ResetAction;
pub use provider_setup::{ProviderSetupStep, jump_to_model_selection, spawn_provider_model_load};

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

#[derive(Debug, Clone)]
pub struct SettingsState {
    pub section: Section,
    pub active_field: usize,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,
    pub success: Option<String>,
    pub pending_reset: Option<ResetAction>,
    pub in_section: bool,
    pub section_list_state: ListState,
    loaded_field: Option<(Section, usize)>,

    // Provider setup wizard state
    pub provider_setup_step: ProviderSetupStep,
    pub provider_setup_provider: ProviderId,
    pub model_picker: model_picker::ModelPickerState,
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
            cursor: 0,
            error: None,
            success: None,
            pending_reset: None,
            in_section: false,
            section_list_state,
            loaded_field: None,
            provider_setup_step: ProviderSetupStep::SelectProvider,
            provider_setup_provider: ProviderId::OpenAi,
            model_picker: model_picker::ModelPickerState::default(),
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
}

fn build_body(state: &AppState) -> Text<'static> {
    let config = match state.config.as_ref() {
        Some(c) => c,
        None => return Text::from("No configuration available. Press Esc to return."),
    };

    if state.settings.section == Section::Provider && state.settings.in_section {
        return Text::from(provider_setup::build_provider_setup_body(state, config));
    }

    let mut lines = vec![];

    let count = state.settings.field_count();
    for i in 0..count {
        let is_active = i == state.settings.active_field;
        let marker = if is_active { "> " } else { "  " };
        let label = fields::field_label(state.settings.section, i);

        if is_active && state.settings.section != Section::Data && state.settings.is_text_field() {
            let value = &state.settings.input;
            let cursor = state.settings.cursor;
            let prefix = format!("{}{}: ", marker, label);
            let before: String = value.chars().take(cursor).collect();
            let at = value.chars().nth(cursor).unwrap_or(' ');
            let after: String = value.chars().skip(cursor + 1).collect();
            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::raw(before),
                Span::styled(
                    at.to_string(),
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after),
            ]));
        } else {
            let value = fields::field_value(config, state.settings.section, i);
            lines.push(Line::from(format!("{}{}: {}", marker, label, value)));
        }
    }

    Text::from(lines)
}

fn build_footer(state: &AppState) -> String {
    if state.settings.section == Section::Provider && state.settings.in_section {
        return provider_setup::build_provider_setup_footer(state);
    }

    let mut lines = vec![String::new()];
    if state.settings.section == Section::Data {
        lines[0] = "↑/↓: action | Enter: reset | Esc: back".to_string();
    } else if state.settings.section == Section::Session {
        lines[0] = "↑/↓: change | Enter: save | Esc: back".to_string();
    } else if state.settings.section == Section::Profile {
        lines[0] = "←/→: move caret | Type: edit | Enter: save | Esc: back".to_string();
    } else {
        lines[0] = "Tab/Shift+Tab: field | Enter: save | Esc: back".to_string();
    }

    if let Some(err) = &state.settings.error {
        lines.push(err.clone());
    }

    if let Some(success) = &state.settings.success {
        lines.push(success.clone());
    }

    lines.join("\n")
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    if let Some(config) = state.config.as_ref() {
        state.settings.ensure_input_loaded(config);
    }

    if let Some(action) = state.settings.pending_reset {
        draw_confirmation(
            frame,
            area,
            "Reset data",
            &format!("Confirm {}", action.label()),
            "y: confirm | any other key: cancel",
        );
        return;
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
            Constraint::Length(2),
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
            Constraint::Length(3),
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

    // The provider wizard's model list renders as a real list widget; every
    // other body is plain text.
    let show_model_list = state.settings.section == Section::Provider
        && state.settings.provider_setup_step == ProviderSetupStep::Model
        && state.settings.model_picker.shows_list();

    if show_model_list {
        let body_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(chunks[1]);
        frame.render_widget(
            Paragraph::new("Select model:").style(Style::default().fg(Color::White)),
            body_chunks[0],
        );
        model_picker::draw_model_list(frame, body_chunks[1], &state.settings.model_picker, None);
    } else {
        frame.render_widget(
            Paragraph::new(build_body(state)).style(Style::default().fg(Color::White)),
            chunks[1],
        );
    }

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
                data::execute_reset(state, action).await?;
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
                state.settings.success = None;
                state.settings.in_section = true;
                if section == Section::Provider {
                    provider_setup::init_provider_setup(state);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    if state.settings.section == Section::Provider {
        return provider_setup::handle_provider_setup_key(state, code).await;
    }

    match code {
        KeyCode::Esc => {
            state.settings.in_section = false;
            state.settings.error = None;
            state.settings.success = None;
        }
        KeyCode::Char('\t') | KeyCode::Tab => {
            state.settings.next_field();
            state.settings.loaded_field = None;
            state.settings.error = None;
            state.settings.success = None;
        }
        KeyCode::BackTab => {
            state.settings.prev_field();
            state.settings.loaded_field = None;
            state.settings.error = None;
            state.settings.success = None;
        }
        KeyCode::Up | KeyCode::Char('k') => match state.settings.section {
            Section::Session => {
                if let Some(config) = state.config.as_mut() {
                    let current = config.preferences.batch_size;
                    config.preferences.batch_size = if current <= 2 { 5 } else { current - 1 };
                }
                state.settings.success = None;
            }
            Section::Data => {
                state.settings.prev_field();
                state.settings.loaded_field = None;
                state.settings.error = None;
                state.settings.success = None;
            }
            _ => {}
        },
        KeyCode::Down | KeyCode::Char('j') => match state.settings.section {
            Section::Session => {
                if let Some(config) = state.config.as_mut() {
                    let current = config.preferences.batch_size;
                    config.preferences.batch_size = if current >= 5 { 2 } else { current + 1 };
                }
                state.settings.success = None;
            }
            Section::Data => {
                state.settings.next_field();
                state.settings.loaded_field = None;
                state.settings.error = None;
                state.settings.success = None;
            }
            _ => {}
        },
        KeyCode::Enter => {
            if state.settings.section == Section::Data {
                if let Some(action) = ResetAction::from_field(state.settings.active_field) {
                    state.settings.pending_reset = Some(action);
                }
            } else if let Some(config) = state.config.as_mut() {
                if let Err(e) = state.settings.save(config, &state.data_dir) {
                    state.settings.error = Some(e.to_string());
                    state.settings.success = None;
                } else {
                    state.settings.error = None;
                    state.settings.success = Some("Saved".to_string());
                }
            }
        }
        KeyCode::Char(c) if state.settings.is_text_field() => {
            insert_char(&mut state.settings.input, &mut state.settings.cursor, c);
            state.settings.success = None;
        }
        KeyCode::Backspace if state.settings.is_text_field() => {
            remove_before(&mut state.settings.input, &mut state.settings.cursor);
            state.settings.success = None;
        }
        KeyCode::Delete if state.settings.is_text_field() => {
            remove_at(&mut state.settings.input, &mut state.settings.cursor);
            state.settings.success = None;
        }
        KeyCode::Left | KeyCode::Char('h')
            if state.settings.is_text_field() && state.settings.cursor > 0 =>
        {
            state.settings.cursor -= 1;
        }
        KeyCode::Right | KeyCode::Char('l') if state.settings.is_text_field() => {
            let len = state.settings.input.chars().count();
            if state.settings.cursor < len {
                state.settings.cursor += 1;
            }
        }
        KeyCode::Home if state.settings.is_text_field() => {
            state.settings.cursor = 0;
        }
        KeyCode::End if state.settings.is_text_field() => {
            state.settings.cursor = state.settings.input.chars().count();
        }
        _ => {}
    }

    Ok(())
}

fn clamp_cursor(input: &str, cursor: &mut usize) {
    let len = input.chars().count();
    if *cursor > len {
        *cursor = len;
    }
}

fn insert_char(input: &mut String, cursor: &mut usize, c: char) {
    clamp_cursor(input, cursor);
    let byte_pos: usize = input.chars().take(*cursor).map(|c| c.len_utf8()).sum();
    input.insert(byte_pos, c);
    *cursor += 1;
}

fn remove_before(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor == 0 {
        return;
    }
    let byte_pos: usize = input.chars().take(*cursor).map(|c| c.len_utf8()).sum();
    let prev_byte_pos: usize = input.chars().take(*cursor - 1).map(|c| c.len_utf8()).sum();
    input.replace_range(prev_byte_pos..byte_pos, "");
    *cursor -= 1;
}

fn remove_at(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor >= input.chars().count() {
        return;
    }
    let byte_pos: usize = input.chars().take(*cursor).map(|c| c.len_utf8()).sum();
    let next_byte_pos: usize = input.chars().take(*cursor + 1).map(|c| c.len_utf8()).sum();
    input.replace_range(byte_pos..next_byte_pos, "");
}
