//! Sync-all progress view: shown right after a successful login while the
//! orchestrator binds and syncs every language pair. Modeled on the model
//! check view: one status row per pair, spinner for the running one, a
//! summary plus a continue hint when finished.

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, View};
use crate::ui::colors;
use crate::ui::labels::{get_sync_all_labels, native_language_code};
use crate::ui::views::settings::account::PairSyncStatus;
use crate::ui::widgets::Toast;
use open_course_core::error::Result;

#[derive(Debug)]
pub struct PairSyncRow {
    pub pair_id: String,
    pub title: String,
    /// `None` — still pending.
    pub status: Option<PairSyncStatus>,
}

#[derive(Debug, Default)]
pub struct SyncAllState {
    pub rows: Vec<PairSyncRow>,
    pub done: bool,
    pub failed: usize,
    /// The finished run was interrupted by the user (Esc).
    pub cancelled: bool,
    pub return_to: Option<View>,
    /// Abort handle of the running sync-all task; `Some` while it runs.
    pub abort: Option<tokio::task::AbortHandle>,
}

/// Seeds the rows from the config and switches to the view. The caller then
/// schedules the actual run (`SyncTrigger::AfterLogin`).
pub fn start(state: &mut AppState) {
    let rows = state
        .config
        .as_ref()
        .map(|c| {
            c.pairs
                .iter()
                .map(|p| PairSyncRow {
                    pair_id: p.id.clone(),
                    title: format!(
                        "{} → {}",
                        p.profile.native_language, p.profile.target_language
                    ),
                    status: None,
                })
                .collect()
        })
        .unwrap_or_default();
    state.sync_all = SyncAllState {
        rows,
        done: false,
        failed: 0,
        cancelled: false,
        return_to: Some(state.view),
        abort: None,
    };
    state.view = View::SyncAll;
}

pub fn apply_progress(state: &mut AppState, pair_id: &str, status: PairSyncStatus) {
    if let Some(row) = state
        .sync_all
        .rows
        .iter_mut()
        .find(|r| r.pair_id == pair_id)
    {
        row.status = Some(status);
    }
}

pub async fn apply_finished(state: &mut AppState, failed: usize, cancelled: bool) {
    state.sync_all.done = true;
    state.sync_all.failed = failed;
    state.sync_all.cancelled = cancelled;
    state.sync_all.abort = None;
    let labels = get_sync_all_labels(native_language_code(state.config.as_ref()));
    let summary = if cancelled {
        labels.summary_cancelled.to_string()
    } else if failed == 0 {
        labels.summary_ok.to_string()
    } else {
        labels
            .summary_failed
            .replace("{failed}", &failed.to_string())
    };
    state.toast = Some(if cancelled || failed > 0 {
        Toast::error(summary)
    } else {
        Toast::info(summary)
    });
    // The account section shows the fresh sync state when it opens next.
    state.settings.account.sync_enabled = state.db.metadata().sync_enabled().await.unwrap_or(false);
    state.settings.account.last_sync_at = state.db.metadata().last_sync_at().await.ok().flatten();
    state.settings.account.outbox_len = state.db.outbox().len().await.ok();
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let labels = get_sync_all_labels(native_language_code(state.config.as_ref()));

    let footer = if state.sync_all.done {
        let summary = if state.sync_all.cancelled {
            labels.summary_cancelled.to_string()
        } else if state.sync_all.failed == 0 {
            labels.summary_ok.to_string()
        } else {
            labels
                .summary_failed
                .replace("{failed}", &state.sync_all.failed.to_string())
        };
        format!("{}\n{}", summary, labels.continue_hint)
    } else {
        format!("{}\n{}", labels.running, labels.cancel_hint)
    };
    let footer_height = footer.lines().count() as u16;
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(labels.title).style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
        ])),
        chunks[0],
    );

    let spinner_symbol = state.spinner.symbol();
    let mut lines: Vec<Line> = Vec::new();
    for row in &state.sync_all.rows {
        lines.push(render_row(row, spinner_symbol, &labels));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), chunks[1]);

    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_row<'a>(
    row: &'a PairSyncRow,
    spinner_symbol: &'a str,
    labels: &crate::ui::labels::SyncAllLabels,
) -> Line<'a> {
    let (marker, style, status_text) = match &row.status {
        None => (
            "·".to_string(),
            Style::default().fg(Color::DarkGray),
            labels.status_pending.to_string(),
        ),
        Some(PairSyncStatus::Running) => (
            spinner_symbol.to_string(),
            Style::default().fg(colors::YELLOW),
            String::new(),
        ),
        Some(PairSyncStatus::Done) => (
            "✓".to_string(),
            Style::default().fg(colors::GREEN),
            labels.status_done.to_string(),
        ),
        Some(PairSyncStatus::Merged(report)) => (
            "✓".to_string(),
            Style::default().fg(colors::GREEN),
            format!(
                "{} ({}/{}/{})",
                labels.status_merged,
                report.topics_merged,
                report.topics_local_only,
                report.topics_cloud_only
            ),
        ),
        Some(PairSyncStatus::Unauthorized) => (
            "✗".to_string(),
            Style::default().fg(Color::Red),
            labels.status_unauthorized.to_string(),
        ),
        Some(PairSyncStatus::Cancelled) => (
            "–".to_string(),
            Style::default().fg(Color::DarkGray),
            labels.status_cancelled.to_string(),
        ),
        Some(PairSyncStatus::Failed(message)) => (
            "✗".to_string(),
            Style::default().fg(Color::Red),
            message.clone(),
        ),
    };
    let mut spans = vec![
        Span::styled(marker, style),
        Span::raw(" "),
        Span::raw(row.title.clone()),
    ];
    if !status_text.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            status_text,
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    if !state.sync_all.done {
        // Esc interrupts the run: the task is aborted, unfinished rows are
        // marked cancelled, and the supervisor finalizes the view with
        // `SyncAllFinished` once the task actually stops.
        if code == KeyCode::Esc && state.sync_all.abort.is_some() {
            if let Some(handle) = state.sync_all.abort.take() {
                handle.abort();
            }
            // A queued trigger must not restart the run the user cancelled.
            state.sync.pending = None;
            for row in &mut state.sync_all.rows {
                if row.status.is_none() || matches!(row.status, Some(PairSyncStatus::Running)) {
                    row.status = Some(PairSyncStatus::Cancelled);
                }
            }
        }
        return Ok(());
    }
    state.view = state.sync_all.return_to.unwrap_or(View::Dashboard);
    state.sync_all.rows.clear();
    state.sync_all.done = false;
    Ok(())
}
