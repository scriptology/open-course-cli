use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde::{Deserialize, Serialize};

use crate::app::{AppState, LlmResult, View};
use crate::db::curriculum::{CEFR_LEVELS, Curriculum, Topic, difficulty_to_cefr};
use crate::error::{AppError, Result};
use crate::llm::client::{DEFAULT_MAX_TOKENS, extract_typed};
use crate::llm::factory::create_llm_model;
use crate::llm::pipeline::generate_curriculum as generate_curriculum_llm;
use crate::llm::prompts::build_curriculum_extension_prompt;
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};
use crate::ui::views::docs;
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};
use crate::ui::widgets::{build_footer, draw_confirmation};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CurriculumSortBy {
    #[default]
    Progression,
    Score,
}

impl CurriculumSortBy {
    fn toggle(self) -> Self {
        match self {
            Self::Progression => Self::Score,
            Self::Score => Self::Progression,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CurriculumState {
    pub topics: Vec<Topic>,
    pub progress: std::collections::HashMap<String, f64>,
    pub list_state: ListState,
    pub loading: bool,
    pub pending_reset: bool,
    pub pending_delete: Option<Topic>,
    pub sort_by: CurriculumSortBy,
}

impl CurriculumState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn load(&mut self, db: &crate::db::Database) -> Result<()> {
        crate::db::curriculum::cleanup_topics(db).await?;
        let curriculum = db.curriculum().read_all().await?;
        let progress = db.progress().read_all().await?;
        let progress_map: std::collections::HashMap<String, f64> = progress
            .topics
            .iter()
            .map(|t| (t.topic_id.clone(), t.score))
            .collect();

        self.topics = curriculum.topics;
        self.progress = progress_map;
        if self.topics.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.sort_topics();
        Ok(())
    }

    pub fn sort_topics(&mut self) {
        match self.sort_by {
            CurriculumSortBy::Progression => self.topics.sort_by(|a, b| {
                let order_a = a.order.unwrap_or(i32::MAX);
                let order_b = b.order.unwrap_or(i32::MAX);
                match order_a.cmp(&order_b) {
                    std::cmp::Ordering::Equal => a.cefr_numeric().cmp(&b.cefr_numeric()),
                    other => other,
                }
            }),
            CurriculumSortBy::Score => self.topics.sort_by(|a, b| {
                let score_a = self.progress.get(&a.id).copied().unwrap_or(0.0);
                let score_b = self.progress.get(&b.id).copied().unwrap_or(0.0);
                match score_a
                    .partial_cmp(&score_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                {
                    std::cmp::Ordering::Equal => a.sort_key().cmp(&b.sort_key()),
                    other => other,
                }
            }),
        }
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    if state.curriculum.loading {
        let labels = get_report_labels(native_language_code(state.config.as_ref()));
        let accent = colors::BLUE;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(area);

        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(Span::styled(
                    labels.curriculum,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ])),
            chunks[0],
        );

        let spinner_symbol = state.spinner.symbol();
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(spinner_symbol, Style::default().fg(colors::YELLOW)),
            Span::raw(" "),
            Span::raw(
                state
                    .stream_status
                    .as_deref()
                    .unwrap_or(labels.loading_curriculum),
            ),
        ]));

        let levels = generation_levels(state);
        for level in levels {
            let status = state
                .curriculum_progress
                .get(&level)
                .cloned()
                .unwrap_or_else(|| "waiting...".to_string());
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(level, Style::default().fg(accent)),
                Span::raw(": "),
                Span::raw(status),
            ]));
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines)).style(Style::default().fg(Color::White)),
            chunks[1],
        );

        frame.render_widget(
            Paragraph::new(build_footer(&[("Esc", labels.cancel)]))
                .style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
        return;
    }

    if state.curriculum.pending_reset {
        draw_confirmation(
            frame,
            area,
            "Regenerate Curriculum",
            "This will delete the current curriculum, all progress scores, and topic reviews.\nAre you sure?",
            "y: confirm | n/Esc: cancel",
        );
        return;
    }

    if let Some(topic) = &state.curriculum.pending_delete {
        draw_confirmation(
            frame,
            area,
            "Delete Topic",
            &format!(
                "Delete \"{}\" and its progress/review?\nThis cannot be undone.",
                topic.name
            ),
            "y: confirm | n/Esc: cancel",
        );
        return;
    }

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let accent = colors::BLUE;
    let header_height = 3;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let mut header_lines = vec![
        Line::from(Span::styled(
            labels.curriculum,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if !state.curriculum.topics.is_empty() {
        let sort_label = match state.curriculum.sort_by {
            CurriculumSortBy::Progression => labels.sort_progression,
            CurriculumSortBy::Score => labels.sort_score,
        };
        header_lines.push(Line::from(Span::styled(
            format!("{}: {}", labels.sort, sort_label),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let header = Text::from(header_lines);
    frame.render_widget(Paragraph::new(header), chunks[0]);

    let items: Vec<ListItem> = if state.curriculum.topics.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            labels.no_curriculum_loaded,
            Style::default().fg(colors::YELLOW),
        )))]
    } else {
        state
            .curriculum
            .topics
            .iter()
            .map(|topic| {
                let score = state
                    .curriculum
                    .progress
                    .get(&topic.id)
                    .copied()
                    .unwrap_or(0.0);
                let fallback = difficulty_to_cefr(&topic.difficulty);
                let level = topic
                    .level
                    .as_deref()
                    .or(fallback.as_deref())
                    .unwrap_or("?");
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} [{}] ", topic.name, level)),
                    Span::styled(format!("[{:.0}]", score), score_style(score)),
                ]))
            })
            .collect()
    };

    let list = List::new(items).highlight_symbol("> ").highlight_style(
        Style::default()
            .fg(colors::BLUE)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(list, chunks[1], &mut state.curriculum.list_state);

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let sort_label = match state.curriculum.sort_by {
        CurriculumSortBy::Progression => labels.sort_progression,
        CurriculumSortBy::Score => labels.sort_score,
    };
    let help = if state.curriculum.topics.is_empty() {
        build_footer(&[("g", labels.generate_label), ("Esc", labels.back)])
    } else {
        let sort_entry = format!("{} ({})", labels.sort, sort_label);
        build_footer(&[
            ("↑↓/wheel", labels.navigate),
            ("Enter", labels.docs),
            ("s", sort_entry.as_str()),
            ("a", labels.add_topics_label),
            ("x", labels.delete_label),
            ("r", labels.reset_label),
            ("Esc", labels.back),
        ])
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.curriculum.pending_reset {
        match code {
            KeyCode::Char('y') => {
                state.curriculum.pending_reset = false;
                reset_and_generate_curriculum(state).await?;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.curriculum.pending_reset = false;
            }
            _ => {}
        }
        return Ok(());
    }

    if state.curriculum.pending_delete.is_some() {
        match code {
            KeyCode::Char('y') => {
                delete_selected_topic(state).await?;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.curriculum.pending_delete = None;
            }
            _ => {}
        }
        return Ok(());
    }

    match code {
        KeyCode::Esc => {
            if state.curriculum.loading {
                state.cancelled = true;
                state.curriculum.loading = false;
            }
            state.view = View::Dashboard;
        }
        KeyCode::Char('j') | KeyCode::Down if !state.curriculum.topics.is_empty() => {
            select_next_wrapping(
                &mut state.curriculum.list_state,
                state.curriculum.topics.len(),
            );
        }
        KeyCode::Char('k') | KeyCode::Up if !state.curriculum.topics.is_empty() => {
            select_previous_wrapping(
                &mut state.curriculum.list_state,
                state.curriculum.topics.len(),
            );
        }
        KeyCode::Char('g') => {
            generate_curriculum(state).await?;
        }
        KeyCode::Enter if state.curriculum.topics.is_empty() && !state.curriculum.loading => {
            generate_curriculum(state).await?;
        }
        KeyCode::Char('r') if !state.curriculum.topics.is_empty() => {
            state.curriculum.pending_reset = true;
        }
        KeyCode::Char('x') if !state.curriculum.topics.is_empty() => {
            let selected = state.curriculum.list_state.selected().unwrap_or(0);
            if let Some(topic) = state.curriculum.topics.get(selected).cloned() {
                state.curriculum.pending_delete = Some(topic);
            }
        }
        KeyCode::Char('a') if !state.curriculum.topics.is_empty() => {
            extend_curriculum(state, 5).await?;
        }
        KeyCode::Char('s') if !state.curriculum.topics.is_empty() => {
            state.curriculum.sort_by = state.curriculum.sort_by.toggle();
            state.curriculum.sort_topics();
        }
        KeyCode::Enter if !state.curriculum.loading && !state.curriculum.topics.is_empty() => {
            let selected = state.curriculum.list_state.selected().unwrap_or(0);
            if let Some(topic) = state.curriculum.topics.get(selected).cloned() {
                docs::load(state).await?;
                if let Some(index) = state.docs.topics.iter().position(|t| t.id == topic.id) {
                    state.docs.list_state.select(Some(index));
                }
                docs::start_viewing(state, topic);
                state.docs.return_to = Some(View::Curriculum);
                state.view = View::Docs;
            }
        }
        _ => {}
    }
    Ok(())
}

fn score_style(score: f64) -> Style {
    if score >= 80.0 {
        Style::default().fg(colors::GREEN)
    } else if score > 0.0 {
        Style::default().fg(colors::YELLOW)
    } else {
        Style::default().fg(colors::BLUE)
    }
}

fn generation_levels(state: &AppState) -> Vec<String> {
    let start = state
        .config
        .as_ref()
        .and_then(|c| c.active_profile().self_assessed_cefr.as_deref())
        .and_then(|c| CEFR_LEVELS.iter().position(|l| *l == c.to_uppercase()))
        .unwrap_or(0);
    CEFR_LEVELS
        .iter()
        .skip(start)
        .map(|l| l.to_string())
        .collect()
}

pub async fn generate_curriculum(state: &mut AppState) -> Result<()> {
    let config = state
        .config
        .clone()
        .ok_or_else(|| AppError::Config("No provider configured".to_string()))?;
    let data_dir = state.data_dir.clone();

    state.curriculum.loading = true;
    state.curriculum.pending_reset = false;
    state.stream_status = Some("Generating curriculum plan...".to_string());
    state.curriculum_progress.clear();

    let tx = state.llm_tx.clone();
    let target_language = config.active_profile().target_language.clone();
    let native_language = config.active_profile().native_language.clone();
    tokio::spawn(async move {
        let result: Result<Curriculum> = async {
            let model = create_llm_model(&config)?;
            let mut curriculum = generate_curriculum_llm(
                model.as_ref(),
                config.active_profile(),
                Some(&tx),
                Some(&data_dir),
            )
            .await?;
            for (index, topic) in curriculum.topics.iter_mut().enumerate() {
                if topic.target_lang.is_empty() {
                    topic.target_lang = target_language.clone();
                }
                if topic.native_lang.is_empty() {
                    topic.native_lang = native_language.clone();
                }
                if topic.version == 0 {
                    topic.version = 1;
                }
                if topic.level.is_none() {
                    topic.level = difficulty_to_cefr(&topic.difficulty);
                }
                if topic.order.is_none() {
                    topic.order = Some(topic.cefr_numeric() * 1000 + index as i32);
                }
            }
            Ok(curriculum)
        }
        .await;
        let _ = tx.send(LlmResult::Curriculum(result)).await;
    });

    Ok(())
}

pub async fn reset_and_generate_curriculum(state: &mut AppState) -> Result<()> {
    state.db.curriculum().reset().await?;
    state.db.progress().reset().await?;
    state.db.reviews().reset().await?;
    state.curriculum.topics.clear();
    state.curriculum.progress.clear();
    state.curriculum.list_state.select(Some(0));
    state.curriculum.sort_by = CurriculumSortBy::default();
    generate_curriculum(state).await?;
    Ok(())
}

async fn delete_selected_topic(state: &mut AppState) -> Result<()> {
    if let Some(topic) = state.curriculum.pending_delete.take() {
        state.db.curriculum().delete_by_topic_id(&topic.id).await?;
        state.db.progress().delete_by_topic_id(&topic.id).await?;
        state.db.reviews().remove_by_topic_id(&topic.id).await?;

        state.curriculum.topics.retain(|t| t.id != topic.id);
        state.curriculum.progress.remove(&topic.id);

        let len = state.curriculum.topics.len();
        let current = state.curriculum.list_state.selected().unwrap_or(0);
        if len == 0 {
            state.curriculum.list_state.select(None);
        } else if current >= len {
            state.curriculum.list_state.select(Some(len - 1));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CurriculumExtension {
    topics: Vec<Topic>,
}

pub async fn extend_curriculum(state: &mut AppState, count: usize) -> Result<()> {
    let config = state
        .config
        .clone()
        .ok_or_else(|| AppError::Config("No provider configured".to_string()))?;

    let curriculum = state.db.curriculum().read_all().await?;
    let progress = state.db.progress().read_all().await?;

    let prompt = build_curriculum_extension_prompt(
        config.active_profile(),
        &curriculum.topics,
        &progress.topics,
        count,
    );

    state.curriculum.loading = true;

    let existing_orders: Vec<i32> = curriculum.topics.iter().filter_map(|t| t.order).collect();
    let base_order = existing_orders.iter().copied().max().unwrap_or(0);

    let tx = state.llm_tx.clone();
    let target_language = config.active_profile().target_language.clone();
    let native_language = config.active_profile().native_language.clone();
    tokio::spawn(async move {
        let result: Result<Vec<Topic>> = async {
            let model = create_llm_model(&config)?;
            let mut extension =
                extract_typed::<CurriculumExtension>(model.as_ref(), &prompt, DEFAULT_MAX_TOKENS)
                    .await?;
            for (index, topic) in extension.topics.iter_mut().enumerate() {
                if topic.target_lang.is_empty() {
                    topic.target_lang = target_language.clone();
                }
                if topic.native_lang.is_empty() {
                    topic.native_lang = native_language.clone();
                }
                if topic.version == 0 {
                    topic.version = 1;
                }
                if topic.level.is_none() {
                    topic.level = difficulty_to_cefr(&topic.difficulty);
                }
                if topic.order.is_none() {
                    topic.order = Some(base_order + 1 + index as i32);
                }
            }
            Ok(extension.topics)
        }
        .await;
        let _ = tx.send(LlmResult::CurriculumExtension(result)).await;
    });

    Ok(())
}
