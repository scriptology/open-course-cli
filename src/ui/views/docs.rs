use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use tokio::sync::mpsc;
use tui_markdown::{Options, from_str_with_options};
use unicode_normalization::UnicodeNormalization;

use crate::app::{AppState, LlmResult, View};
use crate::config::OpenCourseConfig;
use crate::db::Database;
use crate::db::curriculum::Topic;
use crate::db::progress::ProgressTopic;
use crate::db::reviews::TopicReview;
use crate::error::{AppError, Result};
use crate::llm::factory::create_llm_model;
use crate::llm::pipeline::generate_topic_review;
use crate::llm::prompts::build_topic_review_prompt;
use crate::ui::colors;
use crate::ui::labels::{get_docs_labels, native_language_code};
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};
use crate::ui::widgets::OpenCourseStyleSheet;

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

#[derive(Debug, Clone, Default)]
pub struct DocsState {
    pub topics: Vec<Topic>,
    pub list_state: ListState,
    pub sort_by: SortBy,
    pub viewing_topic: Option<Topic>,
    pub content: String,
    pub loading: bool,
    pub saved: bool,
    pub scroll_offset: u16,
    pub max_scroll_offset: u16,
    /// Where to go on Esc from the topic view when docs was opened directly
    /// (e.g. from the curriculum list). `None` means the usual docs list flow.
    pub return_to: Option<crate::app::View>,
}

impl DocsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll_offset as i32;
        self.scroll_offset = (self.scroll_offset as i32 + delta).clamp(0, max) as u16;
    }

    pub fn reset(&mut self) {
        self.topics.clear();
        self.list_state.select(Some(0));
        self.viewing_topic = None;
        self.content.clear();
        self.loading = false;
        self.saved = false;
        self.scroll_offset = 0;
        self.max_scroll_offset = 0;
        self.return_to = None;
    }
}

pub async fn load(state: &mut AppState) -> Result<()> {
    state.docs.loading = true;

    let db = state.db.clone();
    let curriculum = db.curriculum().read_all().await?;
    let progress = db.progress().read_all().await?;

    let touched_ids: std::collections::HashSet<String> = progress
        .topics
        .iter()
        .filter(|p| p.last_practiced.is_some())
        .map(|p| p.topic_id.clone())
        .collect();

    let touched: Vec<Topic> = curriculum
        .topics
        .iter()
        .filter(|t| touched_ids.contains(&t.id))
        .cloned()
        .collect();

    state.docs.topics = if touched.is_empty() {
        curriculum.topics
    } else {
        touched
    };

    let progress_map: HashMap<String, ProgressTopic> = progress
        .topics
        .into_iter()
        .map(|t| (t.topic_id.clone(), t))
        .collect();
    state.docs.loading = false;
    state.docs.list_state.select(Some(0));
    sort_topics(state, &progress_map);
    Ok(())
}

fn sort_topics(state: &mut AppState, progress: &HashMap<String, ProgressTopic>) {
    let sort_by = state.docs.sort_by;
    state.docs.topics.sort_by(|a, b| match sort_by {
        SortBy::Score => {
            let score_a = progress.get(&a.id).map(|p| p.score).unwrap_or(0.0);
            let score_b = progress.get(&b.id).map(|p| p.score).unwrap_or(0.0);
            score_a
                .partial_cmp(&score_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
        SortBy::LastPracticed => {
            let last_a = progress.get(&a.id).and_then(|p| p.last_practiced.as_ref());
            let last_b = progress.get(&b.id).and_then(|p| p.last_practiced.as_ref());
            last_a.cmp(&last_b).reverse()
        }
    });
    let current = state.docs.list_state.selected().unwrap_or(0);
    let new = current.min(state.docs.topics.len().saturating_sub(1));
    state.docs.list_state.select(Some(new));
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let labels = get_docs_labels(native_language_code(state.config.as_ref()));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    if state.docs.viewing_topic.is_some() {
        let topic = state.docs.viewing_topic.clone().unwrap();
        let header = Text::from(vec![
            Line::from(Span::styled(
                labels.title,
                Style::default()
                    .fg(colors::BLUE)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "{} ({}: {})",
                    topic.name,
                    labels.sort,
                    state.docs.sort_by.label()
                ),
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(Paragraph::new(header), chunks[0]);

        let content = state.docs.content.nfc().collect::<String>();
        let loading_message = state.stream_status.as_deref().unwrap_or(labels.loading);
        let body = if state.docs.loading {
            Paragraph::new(loading_message).style(Style::default().fg(colors::YELLOW))
        } else if content.is_empty() {
            Paragraph::new(labels.no_review).style(Style::default().fg(Color::DarkGray))
        } else {
            let options = Options::new(OpenCourseStyleSheet);
            let text = from_str_with_options(&content, &options);
            let paragraph = Paragraph::new(text);
            let line_count = paragraph.line_count(chunks[1].width);
            let max_offset = line_count.saturating_sub(chunks[1].height as usize) as u16;
            state.docs.scroll_offset = state.docs.scroll_offset.min(max_offset);
            state.docs.max_scroll_offset = max_offset;
            paragraph.scroll((state.docs.scroll_offset, 0))
        };

        frame.render_widget(body, chunks[1]);

        let mouse_hint = if state.mouse_capture {
            "wheel: scroll | m: select text"
        } else {
            "mouse: select text | m: wheel scroll"
        };
        let help = if state.docs.content.is_empty() {
            format!(
                "Esc: back to list | e: {} | p: {}",
                labels.regenerate, labels.practice
            )
        } else {
            format!(
                "↑/↓: scroll | {} | Esc: back to list | e: {} | p: {}",
                mouse_hint, labels.regenerate, labels.practice
            )
        };
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    } else {
        let header = Text::from(vec![
            Line::from(Span::styled(
                labels.title,
                Style::default()
                    .fg(colors::BLUE)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "{} ({}: {})",
                    labels.select_topic,
                    labels.sort,
                    state.docs.sort_by.label()
                ),
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(Paragraph::new(header), chunks[0]);

        let loading_message = state.stream_status.as_deref().unwrap_or(labels.loading);
        let body = if state.docs.loading {
            Paragraph::new(loading_message).style(Style::default().fg(colors::YELLOW))
        } else if state.docs.topics.is_empty() {
            Paragraph::new("No topics available.").style(Style::default().fg(Color::DarkGray))
        } else {
            Paragraph::new("")
        };

        frame.render_widget(body, chunks[1]);

        if !(state.docs.loading || state.docs.topics.is_empty()) {
            let items: Vec<ListItem> = state
                .docs
                .topics
                .iter()
                .map(|topic| ListItem::new(format!("{} [{}]", topic.name, topic.difficulty)))
                .collect();
            let list = List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(colors::BLUE)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, chunks[1], &mut state.docs.list_state);
        }

        frame.render_widget(
            Paragraph::new(format!(
                "↑/↓/wheel: navigate | s: sort | Enter: view | p: {} | Esc: back",
                labels.practice
            ))
            .style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.docs.loading && code != KeyCode::Esc {
        return Ok(());
    }

    if state.docs.viewing_topic.is_some() {
        match code {
            KeyCode::Esc => {
                if state.docs.loading {
                    state.cancelled = true;
                }
                state.docs.viewing_topic = None;
                state.docs.content.clear();
                state.docs.loading = false;
                state.docs.scroll_offset = 0;
                if let Some(return_to) = state.docs.return_to.take() {
                    state.view = return_to;
                }
            }
            KeyCode::Char('e') => request_regenerate(state),
            KeyCode::Char('p') => {
                start_practice_from_docs(state).await?;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                state.docs.scroll_by(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.docs.scroll_by(-1);
            }
            _ => {}
        }
    } else {
        match code {
            KeyCode::Esc => {
                if state.docs.loading {
                    state.cancelled = true;
                }
                state.docs.reset();
                state.view = View::Dashboard;
            }
            KeyCode::Char('j') | KeyCode::Down if !state.docs.topics.is_empty() => {
                select_next_wrapping(&mut state.docs.list_state, state.docs.topics.len());
            }
            KeyCode::Char('k') | KeyCode::Up if !state.docs.topics.is_empty() => {
                select_previous_wrapping(&mut state.docs.list_state, state.docs.topics.len());
            }
            KeyCode::Char('s') => {
                state.docs.sort_by = state.docs.sort_by.toggle();
                let db = state.db.clone();
                let progress = db.progress().read_all().await?;
                let progress_map: HashMap<String, ProgressTopic> = progress
                    .topics
                    .into_iter()
                    .map(|t| (t.topic_id.clone(), t))
                    .collect();
                sort_topics(state, &progress_map);
            }
            KeyCode::Enter => {
                let selected = state.docs.list_state.selected().unwrap_or(0);
                if let Some(topic) = state.docs.topics.get(selected).cloned() {
                    start_viewing(state, topic);
                }
            }
            KeyCode::Char('p') => {
                start_practice_from_docs(state).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn start_practice_from_docs(state: &mut AppState) -> Result<()> {
    let selected = state.docs.list_state.selected().unwrap_or(0);
    let topic = state
        .docs
        .topics
        .get(selected)
        .or(state.docs.viewing_topic.as_ref())
        .cloned()
        .ok_or_else(|| AppError::NotFound("Selected topic not found".to_string()))?;

    if state.session.topics.is_empty() {
        state.session.load(&state.db).await?;
    }

    let index = state
        .session
        .topics
        .iter()
        .position(|t| t.id == topic.id)
        .unwrap_or(0);
    state.session.list_state.select(Some(index));

    crate::ui::views::session::start_exercises(state).await?;
    state.view = View::Session;
    Ok(())
}

pub fn start_viewing(state: &mut AppState, topic: Topic) {
    state.docs.viewing_topic = Some(topic);
    state.docs.content.clear();
    state.docs.saved = false;
    state.docs.scroll_offset = 0;
    spawn_generate(state, false);
}

pub fn request_regenerate(state: &mut AppState) {
    if state.docs.viewing_topic.is_some() {
        state.docs.content.clear();
        state.docs.saved = false;
        spawn_generate(state, true);
    }
}

fn spawn_generate(state: &mut AppState, force: bool) {
    let topic = match state.docs.viewing_topic.clone() {
        Some(t) => t,
        None => return,
    };

    let config = match state.config.clone() {
        Some(c) => c,
        None => {
            let _ = state
                .llm_tx
                .try_send(LlmResult::TopicReview(Err(AppError::Config(
                    "No provider configured".to_string(),
                ))));
            state.docs.loading = false;
            return;
        }
    };

    state.docs.loading = true;

    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let result = generate_inner(db, config, topic, force, data_dir, tx.clone()).await;
        let _ = tx.send(LlmResult::TopicReview(result)).await;
    });
}

async fn generate_inner(
    db: Arc<Database>,
    config: OpenCourseConfig,
    topic: Topic,
    force: bool,
    data_dir: std::path::PathBuf,
    tx: mpsc::Sender<LlmResult>,
) -> Result<String> {
    if force {
        db.reviews().remove_by_topic_id(&topic.id).await?;
    } else if let Some(cached) = db.reviews().get_by_topic_id(&topic.id).await? {
        return Ok(cached.content);
    }

    let model = create_llm_model(&config)?;
    let prompt = build_topic_review_prompt(config.active_profile(), &topic);
    let text =
        generate_topic_review(model.as_ref(), &prompt, Some(&tx), Some(data_dir.as_path())).await?;

    if text.trim().is_empty() {
        return Err(AppError::Llm("Generated review is empty".to_string()));
    }

    let review = TopicReview {
        topic_id: topic.id,
        content: text.clone(),
        generated_at: Utc::now().to_rfc3339(),
    };
    db.reviews().upsert(&review).await?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_by_clamps_to_bounds() {
        let mut state = DocsState::default();
        state.max_scroll_offset = 10;

        state.scroll_by(3);
        assert_eq!(state.scroll_offset, 3);

        state.scroll_by(-5);
        assert_eq!(state.scroll_offset, 0);

        state.scroll_by(100);
        assert_eq!(state.scroll_offset, 10);
    }
}
