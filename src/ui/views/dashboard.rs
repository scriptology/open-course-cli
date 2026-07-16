use chrono::Utc;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use crate::app::{AppState, View};
use crate::config::OpenCourseConfig;
use crate::core::dashboard::{
    CourseProgress, DailyActivity, LevelProgress, get_course_progress, get_daily_activity,
    get_progress_by_level,
};
use crate::core::session::{get_due_review_topics, get_weak_review_topics};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::ui::labels::{ReportLabels, get_report_labels, native_language_code};
use crate::ui::views::{docs, session};
use crate::ui::widgets::{ActivityChart, HintBar, Logo, StackedProgressBar};
use crate::ui::colors;

#[derive(Debug, Clone)]
pub struct DashboardState {
    pub profile_native: String,
    pub profile_target: String,
    pub session_count: i32,
    pub due_count: usize,
    pub provider: String,
    pub model: String,
    pub course: CourseProgress,
    pub levels: Vec<LevelProgress>,
    pub activity: Vec<DailyActivity>,
    pub weak_topics: Vec<Topic>,
    pub weak_selected: Option<usize>,
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
            levels: Vec::new(),
            activity: Vec::new(),
            weak_topics: Vec::new(),
            weak_selected: None,
        }
    }
}

impl DashboardState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of weak topics shown in the dashboard block.
    pub fn weak_visible_len(&self) -> usize {
        self.weak_topics.len().min(5)
    }

    /// Moves the weak-topics selector with wrapping. Any arrow press activates
    /// it: down lands on the first row, up on the last.
    pub fn move_weak_selection(&mut self, delta: i32) {
        let len = self.weak_visible_len();
        if len == 0 {
            self.weak_selected = None;
            return;
        }
        let len = len as i32;
        let next = match self.weak_selected {
            None => {
                if delta > 0 {
                    0
                } else {
                    len - 1
                }
            }
            Some(cur) => (cur as i32 + delta).rem_euclid(len),
        };
        self.weak_selected = Some(next as usize);
    }

    pub async fn refresh(
        &mut self,
        db: &crate::db::Database,
        config: Option<&OpenCourseConfig>,
    ) -> Result<()> {
        crate::db::curriculum::cleanup_topics(db).await?;
        let curriculum = db.curriculum().read_all().await?;
        let progress = db.progress().read_all().await?;
        let mut all_history = db.history().read_all().await?;
        all_history.sort_by(|a, b| a.date.cmp(&b.date));

        self.course = get_course_progress(&curriculum, &progress);
        self.levels = get_progress_by_level(&curriculum, &progress);
        self.session_count = progress.session_count;
        self.activity = get_daily_activity(&all_history, &progress, 14, chrono::Local::now().date_naive());

        let cefr = config.and_then(|c| c.active_profile().self_assessed_cefr.as_deref());
        self.due_count = get_due_review_topics(&curriculum.topics, &progress, cefr, Utc::now()).len();
        self.weak_topics = get_weak_review_topics(&curriculum.topics, &progress, Utc::now());

        if let Some(config) = config {
            self.profile_native = config.active_profile().native_language.clone();
            self.profile_target = config.active_profile().target_language.clone();
            self.provider = config.active_provider.as_str().to_string();
            self.model = config
                .providers
                .get(&config.active_provider)
                .map(|p| p.model().to_string())
                .unwrap_or_default();
        }

        self.weak_selected = if self.weak_visible_len() > 0 {
            Some(0)
        } else {
            None
        };

        Ok(())
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let narrow = area.width < 90;
    let middle_height = if narrow {
        Constraint::Min(12)
    } else {
        Constraint::Length(14)
    };

    let weak_constraint = if narrow {
        Constraint::Length(7)
    } else {
        Constraint::Min(4)
    };

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            middle_height,
            weak_constraint,
            Constraint::Length(1),
        ])
        .split(area);

    let top_area = vertical[0];
    let middle_area = vertical[1];
    let weak_area = vertical[2];
    let hint_area = vertical[3];

    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    draw_top(frame, top_area, state, labels, narrow);
    draw_middle(frame, middle_area, state, labels);
    draw_weak_topics(frame, weak_area, state, labels);
    draw_hint_bar(frame, hint_area, state, labels);
}

fn draw_top(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &AppState,
    labels: ReportLabels,
    narrow: bool,
) {
    let block = Block::default().padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    if narrow {
        // On narrow screens put the logo on the left and the profile info in a
        // compact two-column grid on the right.
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Min(0)])
            .split(inner);

        frame.render_widget(Logo, chunks[0]);

        let info_lines = profile_info_lines(state, labels);
        let info_area = chunks[1];
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(info_area);

        let left = Text::from(vec![info_lines[0].clone(), info_lines[1].clone()]);
        let right = Text::from(vec![info_lines[2].clone(), info_lines[3].clone()]);
        frame.render_widget(Paragraph::new(left).alignment(Alignment::Left), cols[0]);
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), cols[1]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(inner);

        frame.render_widget(Logo, chunks[0]);
        frame.render_widget(profile_info(state, labels), chunks[1]);
    }
}

fn profile_info_lines(state: &AppState, labels: ReportLabels) -> Vec<Line<'static>> {
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
        .and_then(|c| c.active_profile().self_assessed_cefr.as_ref())
        .map(|c| {
            Span::styled(
                format!(" | {}: {}", labels.level_label, c),
                Style::default().fg(colors::YELLOW),
            )
        })
        .unwrap_or_else(|| Span::raw(""));

    vec![
        Line::from(vec![
            Span::styled(format!("{}: ", labels.learning), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} → {}", native, target)),
            cefr,
        ]),
        Line::from(vec![
            Span::styled(format!("{}: ", labels.sessions), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(state.dashboard.session_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{}: ", labels.due_topics),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(state.dashboard.due_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled(format!("{}: ", labels.provider_label), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} / {}", provider, model)),
        ]),
    ]
}

fn profile_info(state: &AppState, labels: ReportLabels) -> Paragraph<'static> {
    Paragraph::new(Text::from(profile_info_lines(state, labels))).alignment(Alignment::Right)
}

fn draw_middle(frame: &mut ratatui::Frame, area: Rect, state: &AppState, labels: ReportLabels) {
    let narrow = area.width < 100;
    let chunks = if narrow {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area)
    };

    draw_session_dynamics(frame, chunks[0], state, labels);
    draw_progress(frame, chunks[1], state, labels);
}

const COLOR_NEW: Color = colors::BLUE;
const COLOR_IN_PROGRESS: Color = colors::YELLOW;
const COLOR_COMPLETED: Color = colors::GREEN;

fn draw_progress(frame: &mut ratatui::Frame, area: Rect, state: &AppState, labels: ReportLabels) {
    let block = Block::default()
        .title(labels.progress)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let course = &state.dashboard.course;
    let compact = inner.height < 14;
    let levels = ["A1", "A2", "B1", "B2", "C1", "C2"];

    if compact {
        let mut constraints = vec![Constraint::Length(1), Constraint::Length(1)];
        constraints.extend((0..levels.len()).map(|_| Constraint::Length(1)));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}: ", labels.course_label), Style::default().add_modifier(Modifier::BOLD)),
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

        for (i, level) in levels.iter().enumerate() {
            let progress = state
                .dashboard
                .levels
                .iter()
                .find(|l| l.level == *level)
                .cloned()
                .unwrap_or(LevelProgress {
                    level: (*level).to_string(),
                    total: 0,
                    completed: 0,
                    in_progress: 0,
                    not_started: 0,
                    percent: 0.0,
                });

            let row = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(12), Constraint::Min(0)])
                .split(chunks[2 + i]);

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", level),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(progress.completed.to_string(), Style::default().fg(COLOR_COMPLETED)),
                    Span::raw("/"),
                    Span::styled(progress.in_progress.to_string(), Style::default().fg(COLOR_IN_PROGRESS)),
                    Span::raw("/"),
                    Span::styled(progress.not_started.to_string(), Style::default().fg(COLOR_NEW)),
                ])),
                row[0],
            );

            let level_bar = StackedProgressBar::new(
                progress.not_started as f64,
                progress.in_progress as f64,
                progress.completed as f64,
            );
            frame.render_widget(level_bar, row[1]);
        }
        return;
    }

    let mut constraints = vec![Constraint::Length(1), Constraint::Length(1)];
    for _ in 0..levels.len() {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(1));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{}: ", labels.course_label), Style::default().add_modifier(Modifier::BOLD)),
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

    for (i, level) in levels.iter().enumerate() {
        let progress = state
            .dashboard
            .levels
            .iter()
            .find(|l| l.level == *level)
            .cloned()
            .unwrap_or(LevelProgress {
                level: (*level).to_string(),
                total: 0,
                completed: 0,
                in_progress: 0,
                not_started: 0,
                percent: 0.0,
            });

        let label_idx = 2 + i * 2;
        let bar_idx = 3 + i * 2;

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{}: ", level),
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

        let level_bar = StackedProgressBar::new(
            progress.not_started as f64,
            progress.in_progress as f64,
            progress.completed as f64,
        );
        frame.render_widget(level_bar, chunks[bar_idx]);
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
            .enumerate()
            .map(|(i, topic)| {
                let selected = state.dashboard.weak_selected == Some(i);
                let (marker, name_style) = if selected {
                    (
                        "> ",
                        Style::default()
                            .fg(colors::BLUE)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("• ", Style::default().add_modifier(Modifier::BOLD))
                };
                Line::from(vec![
                    Span::styled(marker, name_style),
                    Span::styled(topic.name.clone(), name_style),
                    Span::styled(
                        format!(
                            " [{}]",
                            topic.level.as_deref().unwrap_or(&topic.difficulty)
                        ),
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

fn draw_hint_bar(frame: &mut ratatui::Frame, area: Rect, state: &AppState, labels: ReportLabels) {
    let mut hints: Vec<(&str, &str)> = vec![
        ("n", labels.start_session),
        ("d", labels.docs),
        ("c", labels.curriculum),
        ("p", labels.pairs),
        ("s", labels.settings),
        ("q", labels.quit),
    ];
    if state.dashboard.weak_visible_len() > 0 {
        hints.insert(0, ("Enter", labels.start_session));
        hints.insert(0, ("↑↓", labels.select_topic));
    }
    frame.render_widget(
        HintBar::new(&hints).model(state.dashboard.model.as_str()),
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
        KeyCode::Char('d') => {
            docs::load(state).await?;
            state.docs.return_to = None;
            state.view = View::Docs;
        }
        KeyCode::Char('c') => state.view = View::Curriculum,
        KeyCode::Char('p') => state.view = View::Pairs,
        KeyCode::Char('s') => state.view = View::Settings,
        KeyCode::Down | KeyCode::Char('j') => state.dashboard.move_weak_selection(1),
        KeyCode::Up | KeyCode::Char('k') => state.dashboard.move_weak_selection(-1),
        KeyCode::Enter => {
            if let Some(sel) = state.dashboard.weak_selected {
                if let Some(topic) = state
                    .dashboard
                    .weak_topics
                    .iter()
                    .take(5)
                    .nth(sel)
                    .cloned()
                {
                    state.view = View::Session;
                    session::start_review_topic_session(state, topic.id).await?;
                }
            }
        }
        KeyCode::Esc => {
            state.dashboard.weak_selected = None;
        }
        _ => {}
    }
    Ok(())
}
