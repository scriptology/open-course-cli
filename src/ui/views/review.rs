use std::collections::HashMap;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::{AppState, View};
use crate::db::curriculum::{Topic, difficulty_to_cefr};
use crate::db::progress::ProgressTopic;
use crate::error::{AppError, Result};
use crate::ui::labels::{get_review_labels, native_language_code};
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    LastPracticed,
    Score,
}

impl SortBy {
    pub fn toggle(self) -> Self {
        match self {
            SortBy::Score => SortBy::LastPracticed,
            SortBy::LastPracticed => SortBy::Score,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Score => "score",
            SortBy::LastPracticed => "last practiced",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewState {
    pub topics: Vec<Topic>,
    pub list_state: ListState,
    pub sort_by: SortBy,
    pub return_to: View,
    pub loading: bool,
    pub progress: HashMap<String, ProgressTopic>,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            topics: Vec::new(),
            list_state: ListState::default(),
            sort_by: SortBy::default(),
            return_to: View::Dashboard,
            loading: false,
            progress: HashMap::new(),
        }
    }
}

impl ReviewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.topics.clear();
        self.list_state.select(Some(0));
        self.progress.clear();
        self.loading = false;
    }
}

pub async fn load(state: &mut AppState) -> Result<()> {
    state.review.loading = true;

    let db = state.db.clone();
    let curriculum = db.curriculum().read_all().await?;
    let progress = db.progress().read_all().await?;

    let progress_map: HashMap<String, ProgressTopic> = progress
        .topics
        .into_iter()
        .map(|t| (t.topic_id.clone(), t))
        .collect();

    let touched_topics: Vec<Topic> = curriculum
        .topics
        .iter()
        .filter(|t| {
            progress_map
                .get(&t.id)
                .map(|p| p.last_practiced.is_some())
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    state.review.topics = touched_topics;
    state.review.progress = progress_map;
    state.review.list_state.select(Some(0));
    sort_topics(state);
    state.review.loading = false;
    Ok(())
}

fn sort_topics(state: &mut AppState) {
    let sort_by = state.review.sort_by;
    let progress = &state.review.progress;
    state.review.topics.sort_by(|a, b| match sort_by {
        SortBy::Score => {
            let score_a = progress.get(&a.id).map(|p| p.score).unwrap_or(0.0);
            let score_b = progress.get(&b.id).map(|p| p.score).unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
        SortBy::LastPracticed => {
            let last_a = progress.get(&a.id).and_then(|p| p.last_practiced.as_ref());
            let last_b = progress.get(&b.id).and_then(|p| p.last_practiced.as_ref());
            last_a.cmp(&last_b).reverse()
        }
    });
    let current = state.review.list_state.selected().unwrap_or(0);
    let new = current.min(state.review.topics.len().saturating_sub(1));
    state.review.list_state.select(Some(new));
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let labels = get_review_labels(native_language_code(state.config.as_ref()));
    let accent = Color::Rgb(0, 122, 255);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let header_lines = vec![
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "Review",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        ratatui::text::Line::from(labels.select_topic),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            match state.review.sort_by {
                SortBy::Score => labels.sort_by_score,
                SortBy::LastPracticed => labels.sort_by_last_practiced,
            },
            Style::default().fg(Color::DarkGray),
        )),
        ratatui::text::Line::from(""),
    ];
    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(header_lines)),
        chunks[0],
    );

    let body = if state.review.loading {
        Paragraph::new("Loading...").style(Style::default().fg(Color::Yellow))
    } else if state.review.topics.is_empty() {
        Paragraph::new("No topics available.").style(Style::default().fg(Color::DarkGray))
    } else {
        Paragraph::new("")
    };

    frame.render_widget(body, chunks[1]);

    if !(state.review.loading || state.review.topics.is_empty()) {
        let items: Vec<ListItem> = state
            .review
            .topics
            .iter()
            .map(|topic| {
                let progress = state.review.progress.get(&topic.id);
                let score = progress.map(|p| p.score).unwrap_or(0.0);
                let last = progress
                    .and_then(|p| p.last_practiced.as_ref())
                    .map(|d| d.split('T').next().unwrap_or(d))
                    .unwrap_or("");
                let level = topic
                    .level
                    .clone()
                    .or_else(|| difficulty_to_cefr(&topic.difficulty))
                    .unwrap_or_else(|| "?".to_string())
                    .to_uppercase();
                ListItem::new(ratatui::text::Line::from(vec![
                    ratatui::text::Span::raw(format!("{} [{}] ", topic.name, level)),
                    ratatui::text::Span::styled(
                        format!("{:.0}", score),
                        score_style(score),
                    ),
                    ratatui::text::Span::styled(
                        if last.is_empty() {
                            String::new()
                        } else {
                            format!(" | {last}")
                        },
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();
        let list = List::new(items).highlight_symbol("> ").highlight_style(
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, chunks[1], &mut state.review.list_state);
    }

    frame.render_widget(
        Paragraph::new(format!(
            "↑/↓: navigate | s: sort | Enter: {} | Esc: back",
            labels.start_review
        ))
        .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn score_style(score: f64) -> Style {
    if score >= 80.0 {
        Style::default().fg(Color::Green)
    } else if score > 0.0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    }
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.review.loading {
        return Ok(());
    }

    match code {
        KeyCode::Esc => {
            let return_to = state.review.return_to;
            state.review.reset();
            state.view = return_to;
        }
        KeyCode::Char('j') | KeyCode::Down if !state.review.topics.is_empty() => {
            select_next_wrapping(&mut state.review.list_state, state.review.topics.len());
        }
        KeyCode::Char('k') | KeyCode::Up if !state.review.topics.is_empty() => {
            select_previous_wrapping(&mut state.review.list_state, state.review.topics.len());
        }
        KeyCode::Char('s') => {
            state.review.sort_by = state.review.sort_by.toggle();
            sort_topics(state);
        }
        KeyCode::Enter if !state.review.topics.is_empty() => {
            start_review_session(state).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn start_review_session(state: &mut AppState) -> Result<()> {
    let selected = state.review.list_state.selected().unwrap_or(0);
    let selected_topic = state
        .review
        .topics
        .get(selected)
        .cloned()
        .ok_or_else(|| AppError::NotFound("Selected topic not found".to_string()))?;

    crate::ui::views::session::start_review_topic_session(state, selected_topic.id).await?;
    state.view = View::Session;
    Ok(())
}
