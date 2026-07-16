use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::config::{OpenCourseConfig, pair_db_path, write_config};
use crate::core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, MentorSession, apply_analysis_to_db, create_session,
};
use crate::db::Database;
use crate::db::curriculum::{Curriculum, Topic, cefr_to_numeric};
use crate::db::learning_items::{LearningItem, is_learning_item_name};
use crate::db::progress::ProgressTopic;
use crate::error::{AppError, Result};
use crate::llm::diagnostics::CheckResult;
use crate::llm::model_listing::ModelInfo;
use crate::llm::pipeline::{generate_topic_metadata, log_debug_event};
use crate::ui::views::{
    CurriculumState, DashboardState, DocsState, ModelCheckState, OnboardingState, PairsState,
    ReportState, SessionState, SettingsState, curriculum, dashboard, docs, model_check,
    onboarding, pairs, report, session, settings,
};
use crate::ui::views::utils::{select_next_wrapping, select_previous_wrapping};
use crate::ui::widgets::{ErrorBox, Spinner};

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
    Quitting,
}

#[derive(Debug)]
pub enum LlmResult {
    Exercises(Result<Vec<Exercise>>),
    Analysis(Result<AnalysisResult>),
    Curriculum(Result<Curriculum>),
    CurriculumExtension(Result<Vec<Topic>>),
    TopicReview(Result<String>),
    Models(Result<Vec<ModelInfo>>),
    OnboardingModels(Result<Vec<ModelInfo>>),
    SimpleText(Result<String>),
    StreamChunk(String),
    CurriculumStreamChunk { level: String, status: String },
    DiagnosticUpdate(CheckResult),
    DiagnosticsDone,
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
    pub quit_requested: Arc<AtomicBool>,
    pub llm_tx: mpsc::Sender<LlmResult>,
    pub spinner: Spinner,
    pub cancelled: bool,
    pub error: Option<String>,
    pub stream_status: Option<String>,
    pub curriculum_progress: std::collections::HashMap<String, String>,
    pub mouse_capture: bool,
}

impl AppState {
    pub async fn new(
        data_dir: PathBuf,
        db: std::sync::Arc<Database>,
        config: Option<OpenCourseConfig>,
        quit_requested: Arc<AtomicBool>,
        llm_tx: mpsc::Sender<LlmResult>,
    ) -> Result<Self> {
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
            quit_requested,
            llm_tx,
            spinner: Spinner::new(),
            cancelled: false,
            error: None,
            stream_status: None,
            curriculum_progress: std::collections::HashMap::new(),
            mouse_capture: true,
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
    let mut state = AppState::new(data_dir, db, config, quit_requested.clone(), llm_tx).await?;
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

    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(100));
    let mut mouse_captured = false;

    loop {
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
            _ = tick.tick() => {
                state.spinner.next();
            }
        }

        if state.view == View::Quitting {
            break;
        }

        let desired_capture = state.mouse_capture && view_supports_mouse(state.view);
        if desired_capture != mouse_captured {
            set_mouse_capture(desired_capture)?;
            mouse_captured = desired_capture;
        }
    }

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
        View::Report => state.report.scroll_by(delta),
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

/// Views where the mouse wheel is useful and there is no text input, so mouse
/// capture can be enabled without breaking typing.
fn view_supports_mouse(view: View) -> bool {
    matches!(view, View::Report | View::Docs | View::Curriculum)
}

fn set_mouse_capture(enabled: bool) -> Result<()> {
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
        set_mouse_capture(enabled)
    }
}

async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match state.view {
        View::Onboarding => onboarding::handle_key(state, code).await,
        View::Dashboard => dashboard::handle_key(state, code).await,
        View::Session => {
            if state.session.topics.is_empty() {
                state.session.load(&state.db).await?;
            }
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
        View::Quitting => Ok(()),
    }
}

fn result_str<T: std::fmt::Debug>(res: &Result<T>) -> String {
    match res {
        Ok(_) => "Ok".to_string(),
        Err(e) => format!("Err({e})"),
    }
}

async fn apply_llm_result(state: &mut AppState, result: LlmResult) {
    let data_dir = state.data_dir.as_path();
    match &result {
        LlmResult::Exercises(res) => log_debug_event(
            "session",
            &format!("apply_llm_result Exercises: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::Analysis(res) => log_debug_event(
            "session",
            &format!("apply_llm_result Analysis: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::Curriculum(res) => log_debug_event(
            "curriculum",
            &format!("apply_llm_result Curriculum: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::CurriculumExtension(res) => log_debug_event(
            "curriculum",
            &format!("apply_llm_result CurriculumExtension: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::TopicReview(res) => log_debug_event(
            "docs",
            &format!("apply_llm_result TopicReview: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::Models(res) => log_debug_event(
            "settings",
            &format!("apply_llm_result Models: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::OnboardingModels(res) => log_debug_event(
            "onboarding",
            &format!("apply_llm_result OnboardingModels: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::SimpleText(res) => log_debug_event(
            "docs",
            &format!("apply_llm_result SimpleText: {}", result_str(res)),
            Some(data_dir),
        ),
        LlmResult::DiagnosticUpdate(res) => log_debug_event(
            "diagnostics",
            &format!("apply_llm_result DiagnosticUpdate: {res:?}"),
            Some(data_dir),
        ),
        LlmResult::DiagnosticsDone => log_debug_event(
            "diagnostics",
            "apply_llm_result DiagnosticsDone",
            Some(data_dir),
        ),
        LlmResult::StreamChunk(_) => {}
        LlmResult::CurriculumStreamChunk { .. } => {}
    }

    if state.cancelled {
        state.cancelled = false;
        clear_loading(state);
        return;
    }

    match result {
        LlmResult::StreamChunk(status) => {
            state.stream_status = Some(status);
        }
        LlmResult::CurriculumStreamChunk { level, status } => {
            state.curriculum_progress.insert(level, status);
        }
        LlmResult::DiagnosticUpdate(check) => {
            if let Some(pos) = state
                .model_check
                .checks
                .iter()
                .position(|c| c.id == check.id)
            {
                state.model_check.checks[pos] = check;
            } else {
                state.model_check.checks.push(check);
            }
        }
        LlmResult::DiagnosticsDone => {
            state.model_check.running = false;
            clear_loading(state);
        }
        LlmResult::Exercises(res) => {
            clear_loading(state);
            match res {
                Ok(exercises) => {
                    let batch_size = state
                        .config
                        .as_ref()
                        .map(|c| c.preferences.batch_size as usize)
                        .unwrap_or(exercises.len());
                    state.session.mentor_session = Some(create_session(exercises, batch_size));
                    state.session.mode = session::Mode::Practicing;
                    state.session.input.clear();
                    state.session.cursor = 0;
                }
                Err(e) => {
                    state.error = Some(e.to_string());
                }
            }
        }
        LlmResult::Analysis(res) => {
            state.session.loading = false;
            state.stream_status = None;
            match res {
                Ok(analysis) => {
                    if let Some(session) = state.session.mentor_session.take() {
                        if let Some(config) = state.config.as_ref() {
                            if let Err(e) = ensure_new_topics(&state.db, &analysis.new_topics).await
                            {
                                state.error = Some(e.to_string());
                                return;
                            }
                            if let Err(e) =
                                ensure_topics_exist(&state.db, config, &session, &state.data_dir)
                                    .await
                            {
                                state.error = Some(e.to_string());
                                return;
                            }
                            if let Err(e) = ensure_progress_for_curriculum(&state.db, config).await
                            {
                                state.error = Some(e.to_string());
                                return;
                            }

                            if let Err(e) = state.session.load(&state.db).await {
                                state.error = Some(e.to_string());
                                return;
                            }
                        }

                        let previous_scores: std::collections::HashMap<String, f64> = state
                            .db
                            .progress()
                            .read_all()
                            .await
                            .map(|p| {
                                p.topics
                                    .into_iter()
                                    .map(|t| (t.topic_id, t.mastery))
                                    .collect()
                            })
                            .unwrap_or_default();

                        let forced_learning_item_ids = state.session.learning_item_ids.clone();
                        let scores_result =
                            apply_analysis_to_db(
                                &analysis,
                                &session,
                                &forced_learning_item_ids,
                                &state.db,
                            ).await;
                        let scores: std::collections::HashMap<String, f64> = match scores_result {
                            Ok(scores) => scores,
                            Err(e) => {
                                state.error = Some(e.to_string());
                                return;
                            }
                        };

                        let evaluated_scores: std::collections::HashMap<&str, f64> = scores
                            .iter()
                            .map(|(id, score)| (id.as_str(), *score))
                            .collect();

                        let weak_topics: Vec<Topic> = state
                            .session
                            .topics
                            .iter()
                            .filter(|t| {
                                evaluated_scores
                                    .get(t.id.as_str())
                                    .copied()
                                    .unwrap_or(100.0)
                                    < 50.0
                            })
                            .cloned()
                            .collect();

                        let report_analysis = AnalysisResult {
                            session_score: analysis.session_score,
                            sentences: analysis.sentences,
                            evaluated_topics: scores
                                .into_iter()
                                .map(|(topic_id, score)| EvaluatedTopic {
                                    previous_score: previous_scores.get(&topic_id).copied(),
                                    topic_id,
                                    score,
                                })
                                .collect(),
                            new_topics: analysis.new_topics.clone(),
                            new_learning_items: analysis.new_learning_items.clone(),
                        };

                        state.report = ReportState {
                            analysis: report_analysis,
                            session,
                            weak_topics,
                            scroll_offset: 0,
                            max_scroll_offset: 0,
                            target_topic_id: state.session.target_topic_id.clone(),
                        };

                        session::reset_session(&mut state.session);
                        state.view = View::Report;
                    }
                }
                Err(e) => {
                    state.error = Some(e.to_string());
                }
            }
        }
        LlmResult::Curriculum(res) => {
            let in_session = state.view == View::Session;
            state.curriculum.loading = false;
            if in_session {
                state.session.loading = false;
            }
            state.stream_status = None;
            match res {
                Ok(curriculum) => {
                    let upsert_result: Result<()> = async {
                        let table = state.db.curriculum();
                        table.delete_all().await?;
                        for topic in &curriculum.topics {
                            table.upsert(topic).await?;
                        }
                        Ok(())
                    }
                    .await;
                    match upsert_result {
                        Ok(()) => {
                            if let Err(e) = state.curriculum.load(&state.db).await {
                                state.error = Some(e.to_string());
                            }
                            if in_session && let Err(e) = state.session.load(&state.db).await {
                                state.error = Some(e.to_string());
                            }
                            if in_session
                                && let Err(e) = session::maybe_start_pending_new_topic(state).await
                            {
                                state.error = Some(e.to_string());
                            }
                        }
                        Err(e) => {
                            state.error = Some(e.to_string());
                        }
                    }
                }
                Err(e) => {
                    if in_session {
                        state.session.pending_new_topic = false;
                    }
                    state.error = Some(e.to_string());
                }
            }
        }
        LlmResult::CurriculumExtension(res) => {
            let in_session = state.view == View::Session;
            state.curriculum.loading = false;
            if in_session {
                state.session.loading = false;
            }
            state.stream_status = None;
            match res {
                Ok(topics) => {
                    let upsert_result: Result<()> = async {
                        let table = state.db.curriculum();
                        for topic in &topics {
                            table.upsert(topic).await?;
                        }
                        Ok(())
                    }
                    .await;
                    match upsert_result {
                        Ok(()) => {
                            if let Err(e) = state.curriculum.load(&state.db).await {
                                state.error = Some(e.to_string());
                            }
                            if in_session && let Err(e) = state.session.load(&state.db).await {
                                state.error = Some(e.to_string());
                            }
                            if in_session
                                && let Err(e) = session::maybe_start_pending_new_topic(state).await
                            {
                                state.error = Some(e.to_string());
                            }
                        }
                        Err(e) => {
                            state.error = Some(e.to_string());
                        }
                    }
                }
                Err(e) => {
                    if in_session {
                        state.session.pending_new_topic = false;
                    }
                    state.error = Some(e.to_string());
                }
            }
        }
        LlmResult::TopicReview(res) => {
            state.docs.loading = false;
            state.stream_status = None;
            match res {
                Ok(text) => {
                    state.docs.content = text;
                    state.docs.saved = true;
                }
                Err(e) => {
                    state.error = Some(e.to_string());
                }
            }
        }
        LlmResult::Models(res) => {
            state.settings.provider_setup_loading = false;
            state.stream_status = None;
            match res {
                Ok(models) => {
                    state.settings.provider_setup_models = models;
                    state.settings.provider_setup_model_selected = 0;
                    state.settings.provider_setup_error = None;
                }
                Err(e) => {
                    state.settings.provider_setup_error = Some(e.to_string());
                }
            }
        }
        LlmResult::OnboardingModels(res) => {
            state.onboarding.model_picker_loading = false;
            state.stream_status = None;
            match res {
                Ok(models) => {
                    state.onboarding.model_picker_models = models;
                    state.onboarding.model_picker_selected = 0;
                    state.onboarding.model_picker_error = None;
                    // If only one model, auto-select it for convenience.
                    if state.onboarding.model_picker_models.len() == 1 {
                        state.onboarding.model = state.onboarding.model_picker_models[0].id.clone();
                        state.onboarding.input = state.onboarding.model.clone();
                    }
                }
                Err(e) => {
                    state.onboarding.model_picker_error = Some(e.to_string());
                }
            }
        }
        LlmResult::SimpleText(_) => {}
    }
}

async fn ensure_topics_exist(
    db: &Database,
    config: &OpenCourseConfig,
    session: &MentorSession,
    data_dir: &std::path::Path,
) -> Result<()> {
    use crate::core::session::unique_topic_ids;

    let curriculum = db.curriculum().read_all().await?;
    let existing_ids: std::collections::HashSet<String> =
        curriculum.topics.iter().map(|t| t.id.clone()).collect();

    let mut missing_ids = std::collections::HashSet::new();
    for exercise in &session.exercises {
        let ids = unique_topic_ids(
            exercise
                .target_topic_ids
                .iter()
                .chain(exercise.side_topic_ids.iter())
                .cloned(),
        );
        for id in ids {
            if !existing_ids.contains(&id) {
                missing_ids.insert(id);
            }
        }
    }

    if missing_ids.is_empty() {
        return Ok(());
    }

    let mut progress = db.progress().read_all().await?;
    let user_cefr = cefr_to_numeric(
        config
            .active_profile()
            .self_assessed_cefr
            .as_deref()
            .unwrap_or("beginner"),
    )
    .unwrap_or(1);

    for topic_id in missing_ids {
        let mut topic = generate_topic_metadata(config, &topic_id, None, Some(data_dir)).await?;

        let topic_cefr = topic.cefr_numeric();
        let initial_score = if topic_cefr > 0 && topic_cefr < user_cefr {
            topic.order = Some(topic_cefr * 1000 - 100);
            100.0
        } else {
            topic.order = Some(topic_cefr * 1000 + 999);
            0.0
        };

        db.curriculum().upsert(&topic).await?;

        if !progress.topics.iter().any(|p| p.topic_id == topic.id) {
            progress.topics.push(crate::db::progress::ProgressTopic {
                topic_id: topic.id,
                score: initial_score,
                mastery: initial_score,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            });
        }
    }

    db.progress().write_all(&progress).await?;
    Ok(())
}

async fn ensure_progress_for_curriculum(db: &Database, config: &OpenCourseConfig) -> Result<()> {
    let curriculum = db.curriculum().read_all().await?;
    let mut progress = db.progress().read_all().await?;

    let existing_ids: std::collections::HashSet<String> =
        progress.topics.iter().map(|t| t.topic_id.clone()).collect();

    let user_cefr = cefr_to_numeric(
        config
            .active_profile()
            .self_assessed_cefr
            .as_deref()
            .unwrap_or("beginner"),
    )
    .unwrap_or(1);

    for topic in &curriculum.topics {
        if existing_ids.contains(&topic.id) {
            continue;
        }
        let topic_cefr = topic.cefr_numeric();
        let initial_score = if topic_cefr > 0 && topic_cefr < user_cefr {
            100.0
        } else {
            0.0
        };
        progress.topics.push(ProgressTopic {
            topic_id: topic.id.clone(),
            score: initial_score,
            mastery: initial_score,
            difficulty_estimate: 0.0,
            practice_count: 0,
            last_practiced: None,
        });
    }

    db.progress().write_all(&progress).await?;
    Ok(())
}

async fn ensure_new_topics(db: &Database, new_topics: &[Topic]) -> Result<()> {
    let mut progress = db.progress().read_all().await?;
    let existing_item_ids: std::collections::HashSet<String> = db
        .learning_items()
        .read_all()
        .await?
        .into_iter()
        .map(|li| li.id)
        .collect();
    for topic in new_topics {
        if is_learning_item_name(&topic.name) {
            let item = LearningItem::from_topic(topic);
            // Do not reset the score of an item that is already being practiced.
            if !existing_item_ids.contains(&item.id) {
                db.learning_items().upsert(&item).await?;
            }
            continue;
        }
        db.curriculum().upsert(topic).await?;
        if !progress.topics.iter().any(|p| p.topic_id == topic.id) {
            progress.topics.push(ProgressTopic {
                topic_id: topic.id.clone(),
                score: 0.0,
                mastery: 0.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            });
        }
    }
    db.progress().write_all(&progress).await?;
    Ok(())
}

fn clear_loading(state: &mut AppState) {
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

    if let Some(err) = &state.error {
        frame.render_widget(ErrorBox::new(err), area);
        return;
    }

    match state.view {
        View::Onboarding => onboarding::draw(frame, area, state),
        View::Dashboard => dashboard::draw(frame, area, state),
        View::Session => session::draw(frame, area, state),
        View::Docs => docs::draw(frame, area, state),
        View::Report => report::draw(frame, area, state),
        View::Curriculum => curriculum::draw(frame, area, state),
        View::Settings => settings::draw(frame, area, state),
        View::ModelCheck => model_check::draw(frame, area, state),
        View::Pairs => pairs::draw(frame, area, state),
        View::Quitting => {}
    }
}
