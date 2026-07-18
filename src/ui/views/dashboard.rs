use chrono::Utc;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap, calendar::Monthly};

use crate::app::{AppState, View};
use crate::config::OpenCourseConfig;
use crate::core::dashboard::{
    CourseProgress, DailyActivity, LevelProgress, calculate_current_level, get_course_progress,
    get_daily_activity, get_progress_by_level,
};
use crate::core::session::{get_due_review_topics, get_weak_review_topics};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::ui::colors;
use crate::ui::labels::{ReportLabels, get_report_labels, native_language_code};
use crate::ui::views::{docs, session};
use crate::ui::widgets::activity_calendar;
use crate::ui::widgets::{HintBar, Logo, StackedProgressBar};

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
    pub scroll_offset: u16,
    pub max_scroll: u16,
    pub current_level: Option<String>,
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
            scroll_offset: 0,
            max_scroll: 0,
            current_level: None,
        }
    }
}

impl DashboardState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the page scroll offset, clamped to the range computed in `draw`.
    pub fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll as i32;
        self.scroll_offset = (self.scroll_offset as i32 + delta).clamp(0, max) as u16;
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
        self.current_level = calculate_current_level(&self.levels);
        self.session_count = progress.session_count;
        self.activity = get_daily_activity(
            &all_history,
            &progress,
            14,
            chrono::Local::now().date_naive(),
        );

        let cefr = config.and_then(|c| c.active_profile().self_assessed_cefr.as_deref());
        self.due_count =
            get_due_review_topics(&curriculum.topics, &progress, cefr, Utc::now()).len();
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
        self.scroll_offset = 0;

        Ok(())
    }
}

const TOP_HEIGHT: u16 = 5;
const PROGRESS_HEIGHT: u16 = 10; // borders(2) + course(1) + gap(1) + 6 levels
const WEAK_HEIGHT: u16 = 7; // borders(2) + up to 5 topics

pub fn draw(frame: &mut ratatui::Frame, area: Rect, state: &mut AppState) {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    // The command bar stays pinned to the bottom of the screen.
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_hint_bar(frame.buffer_mut(), hint_area, state, labels);

    // The page content is sized by its blocks, never squeezed: the activity
    // calendar needs `calendar_height` rows, the progress block 10, the weak
    // block 7. If it exceeds the viewport, it is rendered offscreen and the
    // mouse wheel scrolls the visible window.
    let today = chrono::Local::now().date_naive();
    let calendar_height = activity_calendar::block_height(today);
    let narrow = content_area.width < 90;
    let middle_height = if narrow {
        calendar_height + PROGRESS_HEIGHT
    } else {
        calendar_height.max(PROGRESS_HEIGHT)
    };
    let content_height = TOP_HEIGHT + middle_height + WEAK_HEIGHT;

    if content_height <= content_area.height {
        state.dashboard.scroll_offset = 0;
        state.dashboard.max_scroll = 0;
        let [top_area, middle_area, weak_area] = Layout::vertical([
            Constraint::Length(TOP_HEIGHT),
            Constraint::Length(middle_height),
            Constraint::Min(0),
        ])
        .areas(content_area);
        let buf = frame.buffer_mut();
        draw_top(buf, top_area, state, labels, narrow);
        draw_middle(buf, middle_area, state, labels, calendar_height, narrow);
        draw_weak_topics(buf, weak_area, state, labels);
    } else {
        let max_scroll = content_height - content_area.height;
        state.dashboard.max_scroll = max_scroll;
        state.dashboard.scroll_offset = state.dashboard.scroll_offset.min(max_scroll);

        let mut offscreen = Buffer::empty(Rect::new(0, 0, content_area.width, content_height));
        let [top_area, middle_area, weak_area] = Layout::vertical([
            Constraint::Length(TOP_HEIGHT),
            Constraint::Length(middle_height),
            Constraint::Length(WEAK_HEIGHT),
        ])
        .areas(offscreen.area);
        draw_top(&mut offscreen, top_area, state, labels, narrow);
        draw_middle(
            &mut offscreen,
            middle_area,
            state,
            labels,
            calendar_height,
            narrow,
        );
        draw_weak_topics(&mut offscreen, weak_area, state, labels);

        blit(
            &offscreen,
            frame.buffer_mut(),
            content_area.height,
            state.dashboard.scroll_offset,
        );
    }
}

/// Copies the visible window of the offscreen page into the terminal buffer,
/// leaving the pinned hint row below the viewport untouched.
fn blit(src: &Buffer, dst: &mut Buffer, viewport_height: u16, scroll: u16) {
    let width = dst.area.width.min(src.area.width);
    let height = viewport_height.min(src.area.height.saturating_sub(scroll));
    for y in 0..height {
        for x in 0..width {
            dst[(x, y)] = src[(x, y + scroll)].clone();
        }
    }
}

fn draw_top(buf: &mut Buffer, area: Rect, state: &AppState, labels: ReportLabels, narrow: bool) {
    let block = Block::default().padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    block.render(area, buf);

    if narrow {
        // On narrow screens stack the header vertically: the logo centered on
        // its own row with a gap below, the profile info grid underneath.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        let logo_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(10)])
            .split(rows[0]);
        Logo::new(Alignment::Center).render(logo_row[0], buf);
        Paragraph::new(format!("v{}", env!("CARGO_PKG_VERSION")))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right)
            .render(logo_row[1], buf);

        let info_lines = profile_info_lines(state, labels);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[2]);

        let left = Text::from(vec![info_lines[0].clone(), info_lines[1].clone()]);
        let right = Text::from(vec![info_lines[2].clone(), info_lines[3].clone()]);
        Paragraph::new(left)
            .alignment(Alignment::Left)
            .render(cols[0], buf);
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .render(cols[1], buf);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(inner);

        let left_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(0)])
            .split(chunks[0]);
        Logo::new(Alignment::Left).render(left_chunks[0], buf);
        Paragraph::new(format!("v{}", env!("CARGO_PKG_VERSION")))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Left)
            .render(left_chunks[1], buf);
        profile_info(state, labels).render(chunks[1], buf);
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
        .and_then(|c| {
            state
                .dashboard
                .current_level
                .as_deref()
                .or(c.active_profile().self_assessed_cefr.as_deref())
        })
        .map(|c| {
            Span::styled(
                format!(" | {}: {}", labels.level_label, c),
                Style::default().fg(colors::YELLOW),
            )
        })
        .unwrap_or_else(|| Span::raw(""));

    vec![
        Line::from(vec![
            Span::styled(
                format!("{}: ", labels.learning),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} → {}", native, target)),
            cefr,
        ]),
        Line::from(vec![
            Span::styled(
                format!("{}: ", labels.sessions),
                Style::default().add_modifier(Modifier::BOLD),
            ),
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
            Span::styled(
                format!("{}: ", labels.provider_label),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{} / {}", provider, model)),
        ]),
    ]
}

fn profile_info(state: &AppState, labels: ReportLabels) -> Paragraph<'static> {
    Paragraph::new(Text::from(profile_info_lines(state, labels))).alignment(Alignment::Right)
}

fn draw_middle(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    labels: ReportLabels,
    calendar_height: u16,
    narrow: bool,
) {
    let chunks = if narrow {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(calendar_height), Constraint::Min(0)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area)
    };

    draw_session_dynamics(buf, chunks[0], state, labels);
    draw_progress(buf, chunks[1], state, labels);
}

const COLOR_NEW: Color = colors::BLUE;
const COLOR_IN_PROGRESS: Color = colors::YELLOW;
const COLOR_COMPLETED: Color = colors::GREEN;

fn draw_progress(buf: &mut Buffer, area: Rect, state: &AppState, labels: ReportLabels) {
    let block = Block::default()
        .title(labels.progress)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, buf);

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

        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{}: ", labels.course_label),
                Style::default().add_modifier(Modifier::BOLD),
            ),
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
        ]))
        .render(chunks[0], buf);

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

            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{}: ", level),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    progress.completed.to_string(),
                    Style::default().fg(COLOR_COMPLETED),
                ),
                Span::raw("/"),
                Span::styled(
                    progress.in_progress.to_string(),
                    Style::default().fg(COLOR_IN_PROGRESS),
                ),
                Span::raw("/"),
                Span::styled(
                    progress.not_started.to_string(),
                    Style::default().fg(COLOR_NEW),
                ),
            ]))
            .render(row[0], buf);

            let level_bar = StackedProgressBar::new(
                progress.not_started as f64,
                progress.in_progress as f64,
                progress.completed as f64,
            );
            level_bar.render(row[1], buf);
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

    Paragraph::new(Line::from(vec![
        Span::styled(
            format!("{}: ", labels.course_label),
            Style::default().add_modifier(Modifier::BOLD),
        ),
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
    ]))
    .render(chunks[0], buf);

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
        ]))
        .render(chunks[label_idx], buf);

        let level_bar = StackedProgressBar::new(
            progress.not_started as f64,
            progress.in_progress as f64,
            progress.completed as f64,
        );
        level_bar.render(chunks[bar_idx], buf);
    }
}

fn draw_session_dynamics(buf: &mut Buffer, area: Rect, state: &AppState, labels: ReportLabels) {
    let block = Block::default()
        .title(labels.activity)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, buf);

    // Center the 22-column calendar grid inside the block.
    const CALENDAR_WIDTH: u16 = 22; // 1 gutter + 7 day columns of 3
    let cal_area = Rect {
        x: inner.x + inner.width.saturating_sub(CALENDAR_WIDTH) / 2,
        y: inner.y,
        width: CALENDAR_WIDTH.min(inner.width),
        height: inner.height,
    };

    let today = chrono::Local::now().date_naive();
    let store = activity_calendar::build_event_store(&state.dashboard.activity, today);
    Monthly::new(activity_calendar::chrono_to_time(today), store)
        .default_style(Style::default())
        .show_surrounding(Modifier::DIM)
        .show_month_header(Modifier::BOLD)
        .show_weekdays_header(Style::default().fg(Color::DarkGray))
        .render(cal_area, buf);
}

fn draw_weak_topics(buf: &mut Buffer, area: Rect, state: &AppState, labels: ReportLabels) {
    let block = Block::default()
        .title(labels.weak_topics)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, buf);

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
                        format!(" [{}]", topic.level.as_deref().unwrap_or(&topic.difficulty)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect()
    };

    Paragraph::new(Text::from(content))
        .wrap(Wrap { trim: true })
        .render(inner, buf);
}

fn draw_hint_bar(buf: &mut Buffer, area: Rect, state: &AppState, labels: ReportLabels) {
    let mut hints: Vec<(&str, &str)> = vec![
        ("n", labels.start_next_label),
        ("d", labels.docs),
        ("c", labels.curriculum),
        ("p", labels.pairs),
        ("s", labels.settings),
        ("q", labels.quit),
    ];
    if state.dashboard.weak_visible_len() > 0 {
        hints.insert(0, ("Enter", labels.start_label));
        hints.insert(0, ("↑↓", labels.select_topic));
    }
    HintBar::new(&hints)
        .model(state.dashboard.model.as_str())
        .render(area, buf);
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
                if let Some(topic) = state.dashboard.weak_topics.iter().take(5).nth(sel).cloned() {
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
