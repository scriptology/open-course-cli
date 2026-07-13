use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use crate::app::{AppState, View};
use crate::config::OpenCourseConfig;
use crate::core::dashboard::{
    CourseProgress, DailyActivity, DifficultyProgress, get_course_progress, get_daily_activity,
    get_progress_by_difficulty,
};
use crate::core::session::{get_due_review_topics, get_weak_review_topics};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::ui::labels::{ReportLabels, get_report_labels, native_language_code};
use crate::ui::views::{docs, review, session};
use crate::ui::widgets::{ActivityChart, HintBar, StackedProgressBar, logo};

#[derive(Debug, Clone)]
pub struct DashboardState {
    pub profile_native: String,
    pub profile_target: String,
    pub session_count: i32,
    pub due_count: usize,
    pub provider: String,
    pub model: String,
    pub course: CourseProgress,
    pub difficulty: Vec<DifficultyProgress>,
    pub activity: Vec<DailyActivity>,
    pub weak_topics: Vec<Topic>,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            profile_native: String::new(),
            profile_target: String::new(),
            session_count: 0,
            due_count: 0,
            provider: String::new(),
            model: String::new(),
            course: CourseProgress {
                completed: 0,
                in_progress: 0,
                not_started: 0,
                total: 0,
                percent: 0.0,
            },
            difficulty: Vec::new(),
            activity: Vec::new(),
            weak_topics: Vec::new(),
        }
    }
}

impl DashboardState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn refresh(
        &mut self,
        db: &crate::db::Database,
        config: Option<&OpenCourseConfig>,
    ) -> Result<()> {
        let curriculum = db.curriculum().read_all().await?;
        let progress = db.progress().read_all().await?;
        let mut all_history = db.history().read_all().await?;
        all_history.sort_by(|a, b| a.date.cmp(&b.date));

        self.course = get_course_progress(&curriculum, &progress);
        self.difficulty = get_progress_by_difficulty(&curriculum, &progress);
        self.session_count = progress.session_count;
        self.activity = get_daily_activity(&all_history, &progress, 14, chrono::Local::now().date_naive());

        let cefr = config.and_then(|c| c.profile.self_assessed_cefr.as_deref());
        self.due_count = get_due_review_topics(&curriculum.topics, &progress, cefr).len();
        self.weak_topics = get_weak_review_topics(&curriculum.topics, &progress);

        if let Some(config) = config {
            self.profile_native = config.profile.native_language.clone();
            self.profile_target = config.profile.target_language.clone();
            self.provider = config.active_provider.as_str().to_string();
            self.model = config
                .providers
                .get(&config.active_provider)
                .map(|p| p.model().to_string())
                .unwrap_or_default();
        }

        Ok(())
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(11),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let top_area = vertical[0];
    let middle_area = vertical[1];
    let weak_area = vertical[2];
    let hint_area = vertical[3];

    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    draw_top(frame, top_area, state);
    draw_middle(frame, middle_area, state, labels);
    draw_weak_topics(frame, weak_area, state, labels);
    draw_hint_bar(frame, hint_area, labels, &state.dashboard.model);
}

fn draw_top(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let block = Block::default().padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner);

    frame.render_widget(logo(), chunks[0]);
    frame.render_widget(profile_info(state), chunks[1]);
}

fn profile_info(state: &AppState) -> Paragraph<'static> {
    let config = state.config.as_ref();
    let native = if state.dashboard.profile_native.is_empty() {
        "-".to_string()
    } else {
        state.dashboard.profile_native.clone()
    };
    let target = if state.dashboard.profile_target.is_empty() {
        "-".to_string()
    } else {
        state.dashboard.profile_target.clone()
    };
    let provider = if state.dashboard.provider.is_empty() {
        "-".to_string()
    } else {
        state.dashboard.provider.clone()
    };
    let model = if state.dashboard.model.is_empty() {
        "-".to_string()
    } else {
        state.dashboard.model.clone()
    };
    let cefr = config
        .and_then(|c| c.profile.self_assessed_cefr.as_ref())
        .map(|c| {
            Span::styled(
                format!(" | Level: {}", c),
                Style::default().fg(Color::Yellow),
            )
        })
        .unwrap_or_else(|| Span::raw(""));

    let text = Text::from(vec![
        Line::from(vec![
            Span::styled("Learning: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} → {}", native, target)),
            cefr,
        ]),
        Line::from(vec![
            Span::styled("Sessions: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.dashboard.session_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Due topics: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(state.dashboard.due_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Provider: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} / {}", provider, model)),
        ]),
    ]);

    Paragraph::new(text).alignment(Alignment::Right)
}

fn draw_middle(frame: &mut ratatui::Frame, area: Rect, state: &AppState, labels: ReportLabels) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    draw_session_dynamics(frame, chunks[0], state, labels);
    draw_progress(frame, chunks[1], state, labels);
}

const COLOR_NEW: Color = Color::Rgb(0, 122, 255);
const COLOR_IN_PROGRESS: Color = Color::Yellow;
const COLOR_COMPLETED: Color = Color::Green;

fn draw_progress(frame: &mut ratatui::Frame, area: Rect, state: &AppState, labels: ReportLabels) {
    let block = Block::default()
        .title(labels.progress)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let course = &state.dashboard.course;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Course: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} ({})", course.completed, labels.completed_label),
                Style::default().fg(COLOR_COMPLETED),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} ({})", course.in_progress, labels.in_progress_label),
                Style::default().fg(COLOR_IN_PROGRESS),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} ({})", course.not_started, labels.new_label),
                Style::default().fg(COLOR_NEW),
            ),
        ])),
        chunks[0],
    );

    let bar = StackedProgressBar::new(
        course.not_started as f64,
        course.in_progress as f64,
        course.completed as f64,
    );
    frame.render_widget(bar, chunks[1]);

    let difficulties = ["beginner", "intermediate", "advanced"];

    for (i, difficulty) in difficulties.iter().enumerate() {
        let progress = state
            .dashboard
            .difficulty
            .iter()
            .find(|d| d.difficulty == *difficulty)
            .cloned()
            .unwrap_or(DifficultyProgress {
                difficulty: difficulty.to_string(),
                total: 0,
                completed: 0,
                in_progress: 0,
                not_started: 0,
                percent: 0.0,
            });

        let label_idx = 3 + i * 2;
        let bar_idx = 4 + i * 2;

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{}: ", capitalize(difficulty)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    progress.completed.to_string(),
                    Style::default().fg(COLOR_COMPLETED),
                ),
                Span::raw(" / "),
                Span::styled(
                    progress.in_progress.to_string(),
                    Style::default().fg(COLOR_IN_PROGRESS),
                ),
                Span::raw(" / "),
                Span::styled(
                    progress.not_started.to_string(),
                    Style::default().fg(COLOR_NEW),
                ),
            ])),
            chunks[label_idx],
        );

        let diff_bar = StackedProgressBar::new(
            progress.not_started as f64,
            progress.in_progress as f64,
            progress.completed as f64,
        );
        frame.render_widget(diff_bar, chunks[bar_idx]);
    }
}

fn draw_session_dynamics(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &AppState,
    labels: ReportLabels,
) {
    let block = Block::default()
        .title(labels.activity)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let chart = ActivityChart::new(&state.dashboard.activity).block(block);
    frame.render_widget(chart, area);
}

fn draw_weak_topics(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &AppState,
    labels: ReportLabels,
) {
    let block = Block::default()
        .title(labels.weak_topics)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let content: Vec<Line> = if state.dashboard.weak_topics.is_empty() {
        let message = if state.dashboard.session_count == 0 {
            labels.weak_topics_empty
        } else {
            labels.no_weak_topics
        };
        vec![Line::from(message)]
    } else {
        state
            .dashboard
            .weak_topics
            .iter()
            .take(5)
            .map(|topic| {
                Line::from(vec![
                    Span::raw("• "),
                    Span::styled(
                        topic.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]", topic.difficulty),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect()
    };

    Paragraph::new(Text::from(content))
        .wrap(Wrap { trim: true })
        .render(inner, frame.buffer_mut());
}

fn draw_hint_bar(frame: &mut ratatui::Frame, area: Rect, labels: ReportLabels, model: &str) {
    frame.render_widget(
        HintBar::new(&[
            ("n", labels.start_session),
            ("r", labels.review),
            ("d", labels.docs),
            ("c", labels.curriculum),
            ("s", labels.settings),
            ("q", labels.quit),
        ])
        .model(model),
        area,
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => state.view = View::Quitting,
        KeyCode::Char('n') => {
            if state.dashboard.course.total == 0 {
                state.view = View::Curriculum;
            } else {
                state.view = View::Session;
                session::start_new_topic_session(state).await?;
            }
        }
        KeyCode::Char('r') => {
            review::load(state).await?;
            state.review.return_to = View::Dashboard;
            state.view = View::Review;
        }
        KeyCode::Char('d') => {
            docs::load(state).await?;
            state.view = View::Docs;
        }
        KeyCode::Char('c') => state.view = View::Curriculum,
        KeyCode::Char('s') => state.view = View::Settings,
        _ => {}
    }
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
