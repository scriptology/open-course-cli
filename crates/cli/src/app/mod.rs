mod llm_results;
pub mod sync;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::ui::help;
use crate::ui::labels::{get_report_labels, get_update_labels, native_language_code};
use crate::ui::views::settings::account::{self, SyncMessage};
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};
use crate::ui::views::{
    CurriculumState, DashboardState, DocsState, ModelCheckState, OnboardingState, PairsState,
    ReportState, SessionState, SettingsState, SyncAllState, UpdateState, curriculum, dashboard,
    docs, model_check, onboarding, pairs, report, session, settings, sync_all, update,
};
use crate::ui::widgets::{ErrorBox, HelpOverlay, Spinner, Toast, ToastWidget};
use open_course_config::{OpenCourseConfig, pair_db_path, write_config};
use open_course_core::error::{AppError, Result};
use open_course_db::Database;

use llm_results::apply_llm_result;

pub use open_course_llm::LlmResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Onboarding,
    Dashboard,
    Session,
    Docs,
    Report,
    Curriculum,
    Settings,
    ModelCheck,
    Pairs,
    SyncAll,
    UpdateAvailable,
    Quitting,
}

pub struct AppState {
    pub view: View,
    pub config: Option<OpenCourseConfig>,
    pub data_dir: PathBuf,
    pub db: std::sync::Arc<Database>,
    pub onboarding: OnboardingState,
    pub dashboard: DashboardState,
    pub session: SessionState,
    pub docs: DocsState,
    pub curriculum: CurriculumState,
    pub settings: SettingsState,
    pub report: ReportState,
    pub model_check: ModelCheckState,
    pub pairs: PairsState,
    pub sync_all: SyncAllState,
    pub sync: sync::SyncSchedulerState,
    pub update: UpdateState,
    pub quit_requested: Arc<AtomicBool>,
    pub llm_tx: mpsc::Sender<LlmResult>,
    pub sync_tx: mpsc::Sender<SyncMessage>,
    pub sync_rx: Option<mpsc::Receiver<SyncMessage>>,
    pub spinner: Spinner,
    pub cancelled: bool,
    pub error: Option<String>,
    pub stream_status: Option<String>,
    pub curriculum_progress: std::collections::HashMap<String, String>,
    pub mouse_capture: bool,
    pub help_open: bool,
    pub toast: Option<Toast>,
}

impl AppState {
    pub fn new(
        data_dir: PathBuf,
        db: std::sync::Arc<Database>,
        config: Option<OpenCourseConfig>,
        quit_requested: Arc<AtomicBool>,
        llm_tx: mpsc::Sender<LlmResult>,
    ) -> Result<Self> {
        let (sync_tx, sync_rx) = mpsc::channel::<SyncMessage>(16);
        Ok(Self {
            view: if config.is_some() {
                View::Dashboard
            } else {
                View::Onboarding
            },
            config,
            data_dir,
            db,
            onboarding: OnboardingState::new(),
            dashboard: DashboardState::new(),
            session: SessionState::new(),
            docs: DocsState::new(),
            curriculum: CurriculumState::new(),
            settings: SettingsState::new(),
            report: ReportState::new(),
            model_check: ModelCheckState::new(),
            pairs: PairsState::new(),
            sync_all: SyncAllState::default(),
            sync: sync::SyncSchedulerState::default(),
            update: UpdateState::new(),
            quit_requested,
            llm_tx,
            sync_tx,
            sync_rx: Some(sync_rx),
            spinner: Spinner::new(),
            cancelled: false,
            error: None,
            stream_status: None,
            curriculum_progress: std::collections::HashMap::new(),
            mouse_capture: true,
            help_open: false,
            toast: None,
        })
    }
}

pub async fn run_app(
    terminal: &mut DefaultTerminal,
    data_dir: PathBuf,
    db: std::sync::Arc<Database>,
    config: Option<OpenCourseConfig>,
    quit_requested: Arc<AtomicBool>,
) -> Result<()> {
    let (llm_tx, mut llm_rx) = mpsc::channel::<LlmResult>(16);
    let mut state = AppState::new(data_dir, db, config, quit_requested.clone(), llm_tx)?;

    // Check for a newer release in the background so startup never blocks on
    // the network; the result arrives through the regular event loop.
    {
        let tx = state.llm_tx.clone();
        let data_dir = state.data_dir.clone();
        tokio::spawn(async move {
            let latest = crate::update::latest_release_version().await.ok().flatten();
            open_course_llm::pipeline::log_debug_event(
                "update",
                &format!(
                    "update check: current {}, latest {latest:?}",
                    crate::update::CURRENT_VERSION
                ),
                Some(&data_dir),
            );
            let _ = tx.send(LlmResult::UpdateCheck(latest)).await;
        });
    }

    state
        .dashboard
        .refresh(&state.db, state.config.as_ref())
        .await?;
    if let Err(e) = state.curriculum.load(&state.db).await {
        state.error = Some(e.to_string());
    }
    if state.view == View::Dashboard && state.curriculum.topics.is_empty() {
        state.view = View::Curriculum;
    }

    // Background pull on start: never blocks the UI and silently skips when
    // signed out; pulls every pair with sync enabled.
    sync::schedule(&mut state, sync::SyncTrigger::AppStart).await;

    let mut sync_rx = state.sync_rx.take().expect("sync receiver taken once");

    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(100));
    let mut mouse_captured = false;

    loop {
        // The analysis report lives on the main screen (not in the alternate
        // screen): it is printed once, so the terminal's native scrollback
        // and text selection work. The TUI resumes on any navigation key.
        if state.view == View::Report {
            run_report_screen(terminal, &mut state, &mut events, &mut llm_rx, &mut sync_rx).await?;
            // Mouse capture was released while on the main screen; let the
            // desired-capture check below re-apply it if the new view needs it.
            mouse_captured = false;
            continue;
        }

        terminal.draw(|frame| draw(frame, &mut state))?;

        if state.view == View::Session
            && state.session.pending_new_topic
            && !state.session.loading
            && !state.curriculum.loading
            && state.session.topics.is_empty()
        {
            state.session.loading = true;
            state.session.load(&state.db).await?;
            if state.session.topics.is_empty() {
                curriculum::generate_curriculum(&mut state).await?;
            } else {
                state.session.loading = false;
                if let Err(e) = session::maybe_start_pending_new_topic(&mut state).await {
                    state.error = Some(e.to_string());
                }
            }
        }

        if state.quit_requested.load(Ordering::Relaxed) {
            break;
        }

        tokio::select! {
            Some(event) = events.next() => {
                match event {
                    Ok(event) => handle_event(&mut state, event).await?,
                    Err(e) => {
                        state.error = Some(e.to_string());
                    }
                }
            }
            Some(result) = llm_rx.recv() => {
                apply_llm_result(&mut state, result).await;
            }
            Some(message) = sync_rx.recv() => {
                account::apply_sync_message(&mut state, message).await;
            }
            _ = tick.tick(), if redraw_on_tick(&state) => {
                state.spinner.next();
                if state.toast.as_ref().is_some_and(Toast::expired) {
                    state.toast = None;
                }
            }
        }

        if state.view == View::Quitting {
            break;
        }

        let desired_capture = state.mouse_capture
            && view_supports_mouse(state.view)
            && view_content_overflows(&state);
        if desired_capture != mouse_captured {
            apply_mouse_capture(desired_capture)?;
            mouse_captured = desired_capture;
        }
    }

    Ok(())
}

/// Shows the analysis report on the main screen: leaves the alternate screen,
/// prints the report once (native scrollback + native text selection), then
/// waits for a navigation key. Re-enters the alternate screen before
/// returning. There is no custom scrolling and no mouse capture here.
async fn run_report_screen(
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
    events: &mut EventStream,
    llm_rx: &mut mpsc::Receiver<LlmResult>,
    sync_rx: &mut mpsc::Receiver<SyncMessage>,
) -> Result<()> {
    use ratatui::crossterm::event::DisableMouseCapture;
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};

    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    report::print(state)?;

    while state.view == View::Report {
        tokio::select! {
            Some(event) = events.next() => {
                match event {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c')
                        {
                            state.view = View::Quitting;
                            break;
                        }
                        let previous_view = state.view;
                        if let Err(e) = report::handle_key(state, key.code).await {
                            state.error = Some(e.to_string());
                        }
                        // Errors are rendered by the TUI error box; leave the
                        // main screen so the user actually sees them.
                        if state.error.is_some() {
                            state.view = View::Dashboard;
                        }
                        if state.view == View::Dashboard && previous_view != View::Dashboard {
                            let config = state.config.as_ref();
                            if let Err(e) = state.dashboard.refresh(&state.db, config).await {
                                state.error = Some(e.to_string());
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => state.error = Some(e.to_string()),
                }
            }
            Some(result) = llm_rx.recv() => {
                apply_llm_result(state, result).await;
            }
            Some(message) = sync_rx.recv() => {
                account::apply_sync_message(state, message).await;
            }
        }
        if state.quit_requested.load(Ordering::Relaxed) {
            state.view = View::Quitting;
        }
    }

    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}

async fn handle_event(state: &mut AppState, event: Event) -> Result<()> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                state.view = View::Quitting;
                return Ok(());
            }

            if state.error.is_some() {
                match key.code {
                    KeyCode::Char('q') => state.view = View::Quitting,
                    KeyCode::Char('m') | KeyCode::Char('M') => {
                        state.error = None;
                        settings::jump_to_model_selection(state);
                    }
                    KeyCode::Char('r') => {
                        state.error = None;
                    }
                    _ => state.error = None,
                }
                return Ok(());
            }

            if state.help_open {
                if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
                    state.help_open = false;
                }
                return Ok(());
            }

            if key.code == KeyCode::Char('?') && !is_text_input_active(state) {
                state.help_open = true;
                return Ok(());
            }

            // On mouse-enabled views `m` toggles between wheel-scroll mode and
            // native text-selection mode (capture off).
            if matches!(key.code, KeyCode::Char('m') | KeyCode::Char('M'))
                && view_supports_mouse(state.view)
            {
                let enabled = !state.mouse_capture;
                state.set_mouse_capture(enabled)?;
                return Ok(());
            }

            let previous_view = state.view;
            if let Err(e) = handle_key(state, key.code).await {
                state.error = Some(e.to_string());
            }

            if state.view == View::Dashboard && previous_view != View::Dashboard {
                let config = state.config.as_ref();
                if let Err(e) = state.dashboard.refresh(&state.db, config).await {
                    state.error = Some(e.to_string());
                }
            }
        }
        Event::Resize(_, _) => {
            // The terminal will be redrawn on the next loop iteration.
            // The logo widget checks the allocated area and hides itself
            // automatically when it does not fit.
        }
        Event::Mouse(mouse) => {
            handle_mouse(state, mouse).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_mouse(state: &mut AppState, mouse: MouseEvent) -> Result<()> {
    let down = match mouse.kind {
        MouseEventKind::ScrollDown => true,
        MouseEventKind::ScrollUp => false,
        _ => return Ok(()),
    };
    // Text views scroll by a few lines; list views move the selection one row.
    let delta: i32 = if down { 3 } else { -3 };
    match state.view {
        View::Dashboard => state.dashboard.scroll_by(delta),
        View::Docs if state.docs.viewing_topic.is_some() => state.docs.scroll_by(delta),
        View::Docs => {
            let len = state.docs.topics.len();
            if down {
                select_next_wrapping(&mut state.docs.list_state, len);
            } else {
                select_previous_wrapping(&mut state.docs.list_state, len);
            }
        }
        View::Curriculum => {
            let len = state.curriculum.topics.len();
            if down {
                select_next_wrapping(&mut state.curriculum.list_state, len);
            } else {
                select_previous_wrapping(&mut state.curriculum.list_state, len);
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_text_input_active(state: &AppState) -> bool {
    match state.view {
        View::Session => matches!(state.session.mode, session::Mode::Practicing),
        View::Onboarding => state.onboarding.is_text_step_active(),
        View::Settings => state.settings.is_text_input_active(),
        _ => false,
    }
}

/// Views where the mouse wheel is useful and there is no text input, so mouse
/// capture can be enabled without breaking typing. The report view is
/// excluded on purpose: it is printed to the main screen, where scrolling and
/// text selection are handled natively by the terminal.
fn view_supports_mouse(view: View) -> bool {
    matches!(view, View::Dashboard | View::Docs | View::Curriculum)
}

/// Whether the current view's content is taller than its viewport. Mouse
/// capture is enabled only then: while the terminal owns the mouse, native
/// drag-selection is impossible, so capturing on content that fits the screen
/// would block text selection for no benefit. The overflow flags are computed
/// during `draw`, which runs before this check in the event loop.
fn view_content_overflows(state: &AppState) -> bool {
    match state.view {
        View::Dashboard => state.dashboard.max_scroll > 0,
        View::Docs if state.docs.viewing_topic.is_some() => state.docs.max_scroll_offset > 0,
        View::Docs => state.docs.list_overflows,
        View::Curriculum => state.curriculum.list_overflows,
        _ => false,
    }
}

/// Whether the 100ms tick needs to trigger a redraw: only while a view is
/// waiting on the LLM (spinner animation). Otherwise the tick is skipped and
/// the terminal is not redrawn in idle.
fn redraw_on_tick(state: &AppState) -> bool {
    state.session.loading
        || state.docs.loading
        || state.curriculum.loading
        || state.model_check.running
        || (state.view == View::SyncAll && !state.sync_all.done)
        || state.toast.is_some()
}

/// Applies the desired capture mode to the terminal. Unlike
/// `AppState::set_mouse_capture` it does not touch the `mouse_capture`
/// preference flag — the event loop derives the desired mode from it per view.
fn apply_mouse_capture(enabled: bool) -> Result<()> {
    use ratatui::crossterm::{
        event::{DisableMouseCapture, EnableMouseCapture},
        execute,
    };
    if enabled {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    } else {
        execute!(std::io::stdout(), DisableMouseCapture)?;
    }
    Ok(())
}

impl AppState {
    pub fn set_mouse_capture(&mut self, enabled: bool) -> Result<()> {
        self.mouse_capture = enabled;
        apply_mouse_capture(enabled)
    }
}

async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match state.view {
        View::Onboarding => onboarding::handle_key(state, code).await,
        View::Dashboard => dashboard::handle_key(state, code).await,
        View::Session => {
            state.session.load(&state.db).await?;
            session::handle_key(state, code).await
        }
        View::Docs => docs::handle_key(state, code).await,
        View::Report => report::handle_key(state, code).await,
        View::Curriculum => {
            if state.curriculum.topics.is_empty() {
                state.curriculum.load(&state.db).await?;
            }
            curriculum::handle_key(state, code).await
        }
        View::Settings => settings::handle_key(state, code).await,
        View::ModelCheck => model_check::handle_key(state, code).await,
        View::Pairs => pairs::handle_key(state, code).await,
        View::SyncAll => sync_all::handle_key(state, code).await,
        View::UpdateAvailable => handle_update_key(state, code).await,
        View::Quitting => Ok(()),
    }
}

async fn handle_update_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            run_installer_and_exit(state).await?;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Enter => {
            continue_to_app(state).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn run_installer_and_exit(state: &mut AppState) -> Result<()> {
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
    use std::io::{Write, stdout};

    let update_labels = get_update_labels(native_language_code(state.config.as_ref()));
    let latest = state
        .update
        .latest_version
        .clone()
        .unwrap_or_else(|| update_labels.latest_placeholder.to_string());

    // Restore the normal terminal before handing control to the installer.
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    stdout().flush()?;

    println!("{}", update_labels.installing.replace("{latest}", &latest));

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(crate::update::install_command())
        .status()?;

    if !status.success() {
        ratatui::crossterm::terminal::enable_raw_mode()?;
        state.error = Some(update_labels.installer_failed.to_string());
        return Ok(());
    }

    state.view = View::Quitting;
    Ok(())
}

async fn continue_to_app(state: &mut AppState) -> Result<()> {
    // Keep `latest_version` so the dashboard can keep showing the update badge
    // after the user skips the prompt.
    if state.config.is_some() {
        state.view = View::Dashboard;
        state
            .dashboard
            .refresh(&state.db, state.config.as_ref())
            .await?;
        if let Err(e) = state.curriculum.load(&state.db).await {
            state.error = Some(e.to_string());
        }
        if state.view == View::Dashboard && state.curriculum.topics.is_empty() {
            state.view = View::Curriculum;
        }
    } else {
        state.view = View::Onboarding;
    }
    Ok(())
}

pub(crate) fn clear_loading(state: &mut AppState) {
    state.session.loading = false;
    state.docs.loading = false;
    state.curriculum.loading = false;
    state.stream_status = None;
    state.curriculum_progress.clear();
}

pub async fn switch_pair(state: &mut AppState, pair_id: &str) -> Result<()> {
    let config = state
        .config
        .as_mut()
        .ok_or_else(|| AppError::Config("No config available".to_string()))?;
    config.active_pair = pair_id.to_string();
    write_config(config, &state.data_dir)?;

    let db_path = pair_db_path(&state.data_dir, pair_id);
    let db = Database::connect(&db_path).await?;
    state.db = Arc::new(db);

    clear_loading(state);
    state.curriculum.topics.clear();
    state.curriculum.progress.clear();
    state.curriculum.list_state.select(Some(0));

    state
        .dashboard
        .refresh(&state.db, state.config.as_ref())
        .await?;
    state.curriculum.load(&state.db).await?;

    if state.curriculum.topics.is_empty() {
        state.view = View::Curriculum;
    } else {
        state.view = View::Dashboard;
    }
    Ok(())
}

fn draw(frame: &mut ratatui::Frame, state: &mut AppState) {
    let area = frame.area();
    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    if let Some(err) = &state.error {
        frame.render_widget(
            ErrorBox::new(err, labels.error, labels.error_footer_hint),
            area,
        );
        return;
    }

    match state.view {
        View::Onboarding => onboarding::draw(frame, area, state),
        View::Dashboard => dashboard::draw(frame, area, state),
        View::Session => session::draw(frame, area, state),
        View::Docs => docs::draw(frame, area, state),
        // The report is printed to the main screen by `run_report_screen`;
        // the TUI never draws it.
        View::Report => {}
        View::Curriculum => curriculum::draw(frame, area, state),
        View::Settings => settings::draw(frame, area, state),
        View::ModelCheck => model_check::draw(frame, area, state),
        View::Pairs => pairs::draw(frame, area, state),
        View::SyncAll => sync_all::draw(frame, area, state),
        View::UpdateAvailable => update::draw(frame, area, state),
        View::Quitting => {}
    }

    if state.help_open {
        frame.render_widget(
            HelpOverlay::new(
                &help::groups_for(state),
                labels.help_title,
                labels.close_hint,
            ),
            area,
        );
    }

    if let Some(toast) = &state.toast {
        frame.render_widget(ToastWidget::new(toast, labels), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> AppState {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::connect(&dir.path().join("db")).await.unwrap();
        let (tx, _rx) = mpsc::channel(1);
        AppState::new(
            dir.path().to_path_buf(),
            Arc::new(db),
            None,
            Arc::new(AtomicBool::new(false)),
            tx,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn capture_only_when_content_overflows() {
        let mut state = test_state().await;

        state.view = View::Dashboard;
        assert!(!view_content_overflows(&state));
        state.dashboard.max_scroll = 3;
        assert!(view_content_overflows(&state));

        state.view = View::Report;
        // The report is printed to the main screen; it never captures the
        // mouse, no matter how long the content is.
        assert!(!view_content_overflows(&state));
        assert!(!view_supports_mouse(state.view));

        state.view = View::Docs;
        assert!(!view_content_overflows(&state));
        state.docs.list_overflows = true;
        assert!(view_content_overflows(&state));

        state.view = View::Curriculum;
        assert!(!view_content_overflows(&state));
        state.curriculum.list_overflows = true;
        assert!(view_content_overflows(&state));

        state.view = View::Session;
        assert!(!view_content_overflows(&state));
    }
}
