use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, LlmResult, View};
use crate::config::OpenCourseConfig;
use crate::error::Result;
use crate::llm::diagnostics::{
    CheckResult, CheckStatus, model_check_verdict, run_model_diagnostics,
};
use crate::llm::factory::create_llm_model;
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};

#[derive(Debug, Clone, Default)]
pub struct ModelCheckState {
    pub checks: Vec<CheckResult>,
    pub running: bool,
    pub return_to: Option<View>,
    pub pending_config: Option<OpenCourseConfig>,
}

impl ModelCheckState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_return_to(return_to: View) -> Self {
        Self {
            return_to: Some(return_to),
            ..Self::default()
        }
    }
}

pub fn start(state: &mut AppState, config: OpenCourseConfig, return_to: View) {
    state.model_check = ModelCheckState {
        checks: vec![
            CheckResult {
                id: "connectivity",
                label: "Connectivity".to_string(),
                status: CheckStatus::Pending,
                duration_ms: 0,
                reasoning_ratio: None,
            },
            CheckResult {
                id: "streaming",
                label: "Streaming".to_string(),
                status: CheckStatus::Pending,
                duration_ms: 0,
                reasoning_ratio: None,
            },
            CheckResult {
                id: "exercises",
                label: "Exercise generation".to_string(),
                status: CheckStatus::Pending,
                duration_ms: 0,
                reasoning_ratio: None,
            },
            CheckResult {
                id: "analysis",
                label: "Answer analysis".to_string(),
                status: CheckStatus::Pending,
                duration_ms: 0,
                reasoning_ratio: None,
            },
            CheckResult {
                id: "topic_review",
                label: "Topic review".to_string(),
                status: CheckStatus::Pending,
                duration_ms: 0,
                reasoning_ratio: None,
            },
        ],
        running: true,
        return_to: Some(return_to),
        pending_config: Some(config.clone()),
    };
    state.view = View::ModelCheck;
    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let client = match create_llm_model(&config) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.try_send(LlmResult::DiagnosticUpdate(CheckResult {
                    id: "connectivity",
                    label: "Connectivity".to_string(),
                    status: CheckStatus::Failed(e.to_string()),
                    duration_ms: 0,
                    reasoning_ratio: None,
                }));
                let _ = tx.try_send(LlmResult::DiagnosticsDone);
                return;
            }
        };
        let profile = config.active_profile().clone();
        let _ = run_model_diagnostics(client, &profile, |check| {
            let _ = tx.try_send(LlmResult::DiagnosticUpdate(check));
        })
        .await;
        let _ = tx.try_send(LlmResult::DiagnosticsDone);
    });
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let footer = if state.model_check.running {
        "Running checks...".to_string()
    } else {
        let (has_failed, has_warning) = model_check_verdict(&state.model_check.checks);
        let verdict = if has_failed {
            "Some checks failed. You can change the model or continue anyway."
        } else if has_warning {
            "Model works, but shows warnings."
        } else {
            "Model is ready."
        };
        format!(
            "{} | Enter/c: continue | Esc/b: back to model list | r: retry | s: skip",
            verdict
        )
    };
    let footer_height = footer.lines().count() as u16;
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .split(area);

    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(labels.model_diagnostics)
                .style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
        ])),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    if state.model_check.checks.is_empty() {
        lines.push(Line::from("Running diagnostics...").style(Style::default().fg(colors::YELLOW)));
    } else {
        let spinner_symbol = state.spinner.symbol();
        for check in &state.model_check.checks {
            lines.push(render_check_line(check, spinner_symbol));
        }
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), chunks[1]);

    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_check_line<'a>(check: &'a CheckResult, spinner_symbol: &'a str) -> Line<'a> {
    let verdict = check.verdict(Some(spinner_symbol));
    let mut spans = vec![
        Span::styled(verdict, status_style(&check.status)),
        Span::raw(" "),
        Span::raw(check.label.clone()),
    ];
    if !matches!(check.status, CheckStatus::Pending | CheckStatus::InProgress) {
        spans.push(Span::raw(format!(
            " ({:.1}s)",
            check.duration_ms as f64 / 1000.0
        )));
    }
    if let Some(ratio) = check.reasoning_ratio {
        spans.push(Span::raw(format!(", {:.0}% reasoning", ratio * 100.0)));
    }
    if let Some(msg) = check.status.message() {
        spans.push(Span::raw("\n  "));
        spans.push(Span::styled(
            msg.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn status_style(status: &CheckStatus) -> Style {
    match status {
        CheckStatus::Pending => Style::default().fg(Color::DarkGray),
        CheckStatus::InProgress => Style::default().fg(colors::YELLOW),
        CheckStatus::Passed => Style::default().fg(colors::GREEN),
        CheckStatus::Failed(_) => Style::default().fg(Color::Red),
        CheckStatus::Warning(_) => Style::default().fg(colors::YELLOW),
    }
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if state.model_check.running && !matches!(code, KeyCode::Esc) {
        return Ok(());
    }

    match code {
        KeyCode::Esc | KeyCode::Char('b') => {
            if let Some(return_to) = state.model_check.return_to {
                state.view = return_to;
            }
            state.model_check.running = false;
            state.model_check.checks.clear();
        }
        KeyCode::Char('r') => {
            let return_to = state.model_check.return_to.unwrap_or(View::Dashboard);
            if let Some(config) = state
                .model_check
                .pending_config
                .clone()
                .or_else(|| state.config.clone())
            {
                start(state, config, return_to);
            }
        }
        KeyCode::Char('s') | KeyCode::Enter | KeyCode::Char('c') => {
            finish_model_check(state).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn finish_model_check(state: &mut AppState) -> Result<()> {
    if state.model_check.return_to == Some(View::Onboarding) {
        crate::ui::views::onboarding::finish_onboarding(state).await?;
    } else {
        state.view = View::Dashboard;
        state.settings.reset_to_section_list();
    }
    state.model_check.running = false;
    state.model_check.checks.clear();
    Ok(())
}
