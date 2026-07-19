use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::{AppState, View};
use crate::error::Result;
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};
use crate::ui::views::onboarding;

#[derive(Debug, Clone, Default)]
pub struct PairsState {
    pub selected: usize,
}

impl PairsState {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let accent = colors::BLUE;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                labels.language_pairs,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ])),
        chunks[0],
    );

    let empty = Vec::new();
    let pairs = state.config.as_ref().map(|c| &c.pairs).unwrap_or(&empty);
    let active_id = state
        .config
        .as_ref()
        .map(|c| c.active_pair.as_str())
        .unwrap_or("");

    let list_width = chunks[1].width as usize;
    let items: Vec<ListItem> = pairs
        .iter()
        .enumerate()
        .map(|(_i, pair)| {
            let is_active = pair.id == active_id;
            let label = format!(
                "{} → {}",
                pair.profile.native_language, pair.profile.target_language
            );
            let style = if is_active {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            if is_active {
                let suffix = format!(" [{}]", labels.current);
                let padding =
                    list_width.saturating_sub(label.chars().count() + suffix.chars().count());
                let line = format!("{}{}{}", label, " ".repeat(padding), suffix);
                ListItem::new(Line::from(Span::styled(line, style)))
            } else {
                ListItem::new(Line::from(Span::styled(label, style)))
            }
        })
        .collect();

    let list = List::new(items)
        .highlight_symbol("> ")
        .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD));

    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(state.pairs.selected));
    frame.render_stateful_widget(list, chunks[1], &mut list_state);

    let hint = format!(
        "↑/↓: {} | Enter: {} | a: {} | Esc: {}",
        labels.navigate, labels.switch, labels.add_pair, labels.back
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let pairs = state.config.as_ref().map(|c| c.pairs.len()).unwrap_or(0);
    if pairs == 0 && !matches!(code, KeyCode::Esc | KeyCode::Char('a')) {
        return Ok(());
    }

    match code {
        KeyCode::Esc => state.view = View::Dashboard,
        KeyCode::Char('a') => {
            state.onboarding = onboarding::OnboardingState::for_add_pair();
            state.view = View::Onboarding;
        }
        KeyCode::Up | KeyCode::Char('k') if pairs > 0 => {
            state.pairs.selected = if state.pairs.selected == 0 {
                pairs - 1
            } else {
                state.pairs.selected - 1
            };
        }
        KeyCode::Down | KeyCode::Char('j') if pairs > 0 => {
            state.pairs.selected = (state.pairs.selected + 1) % pairs;
        }
        KeyCode::Enter if pairs > 0 => {
            if let Some(pair) = state
                .config
                .as_ref()
                .and_then(|c| c.pairs.get(state.pairs.selected))
            {
                let id = pair.id.clone();
                if id
                    != state
                        .config
                        .as_ref()
                        .map(|c| c.active_pair.as_str())
                        .unwrap_or("")
                {
                    crate::app::switch_pair(state, &id).await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
