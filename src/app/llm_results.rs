//! Handlers for the results of background LLM tasks, one function per
//! `LlmResult` variant. `apply_llm_result` is the thin dispatcher called by
//! the event loop.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::app::{AppState, LlmResult, View, clear_loading};
use crate::config::OpenCourseConfig;
use crate::core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, MASTERY_THRESHOLD, MentorSession,
    apply_analysis_to_db, create_session, unique_topic_ids,
};
use crate::db::Database;
use crate::db::curriculum::{Curriculum, Topic, cefr_to_numeric};
use crate::db::learning_items::{LearningItem, is_learning_item_name};
use crate::db::progress::{ProgressTopic, initial_topic_score};
use crate::error::Result;
use crate::llm::diagnostics::CheckResult;
use crate::llm::factory::create_llm_model;
use crate::llm::model_listing::ModelInfo;
use crate::llm::pipeline::{generate_topic_metadata, log_debug_event};
use crate::ui::views::{ReportState, session};
use crate::ui::widgets::Toast;

pub async fn apply_llm_result(state: &mut AppState, result: LlmResult) {
    if let Some((tag, message)) = debug_describe(&result) {
        log_debug_event(tag, &message, Some(state.data_dir.as_path()));
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
        LlmResult::DiagnosticUpdate(check) => handle_diagnostic_update(state, check),
        LlmResult::DiagnosticsDone => {
            state.model_check.running = false;
            clear_loading(state);
        }
        LlmResult::Exercises(res) => handle_exercises(state, res),
        LlmResult::Analysis(res) => handle_analysis(state, res).await,
        LlmResult::Curriculum(res) => handle_curriculum(state, res).await,
        LlmResult::CurriculumExtension(res) => handle_curriculum_extension(state, res).await,
        LlmResult::TopicReview(res) => handle_topic_review(state, res),
        LlmResult::Models(res) => {
            state.stream_status = None;
            state.settings.model_picker.apply_result(res);
        }
        LlmResult::OnboardingModels(res) => handle_onboarding_models(state, res),
        LlmResult::SimpleText(_) => {}
    }
}

/// Debug-log description of an incoming result, or `None` for high-frequency
/// stream chunks that are not worth logging.
fn debug_describe(result: &LlmResult) -> Option<(&'static str, String)> {
    let (tag, message) = match result {
        LlmResult::Exercises(res) => (
            "session",
            format!("apply_llm_result Exercises: {}", result_str(res)),
        ),
        LlmResult::Analysis(res) => (
            "session",
            format!("apply_llm_result Analysis: {}", result_str(res)),
        ),
        LlmResult::Curriculum(res) => (
            "curriculum",
            format!("apply_llm_result Curriculum: {}", result_str(res)),
        ),
        LlmResult::CurriculumExtension(res) => (
            "curriculum",
            format!("apply_llm_result CurriculumExtension: {}", result_str(res)),
        ),
        LlmResult::TopicReview(res) => (
            "docs",
            format!("apply_llm_result TopicReview: {}", result_str(res)),
        ),
        LlmResult::Models(res) => (
            "settings",
            format!("apply_llm_result Models: {}", result_str(res)),
        ),
        LlmResult::OnboardingModels(res) => (
            "onboarding",
            format!("apply_llm_result OnboardingModels: {}", result_str(res)),
        ),
        LlmResult::SimpleText(res) => (
            "docs",
            format!("apply_llm_result SimpleText: {}", result_str(res)),
        ),
        LlmResult::DiagnosticUpdate(res) => (
            "diagnostics",
            format!("apply_llm_result DiagnosticUpdate: {res:?}"),
        ),
        LlmResult::DiagnosticsDone => (
            "diagnostics",
            "apply_llm_result DiagnosticsDone".to_string(),
        ),
        LlmResult::StreamChunk(_) | LlmResult::CurriculumStreamChunk { .. } => return None,
    };
    Some((tag, message))
}

fn result_str<T: std::fmt::Debug>(res: &Result<T>) -> String {
    match res {
        Ok(_) => "Ok".to_string(),
        Err(e) => format!("Err({e})"),
    }
}

fn handle_diagnostic_update(state: &mut AppState, check: CheckResult) {
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

fn handle_exercises(state: &mut AppState, res: Result<Vec<Exercise>>) {
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
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

async fn handle_analysis(state: &mut AppState, res: Result<AnalysisResult>) {
    state.session.loading = false;
    state.stream_status = None;
    match res {
        Ok(analysis) => {
            if let Some(session) = state.session.mentor_session.take() {
                if let Some(config) = state.config.as_ref() {
                    if let Err(e) = ensure_new_topics(&state.db, &analysis.new_topics).await {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }
                    if let Err(e) =
                        ensure_topics_exist(&state.db, config, &session, &state.data_dir).await
                    {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }
                    if let Err(e) = ensure_progress_for_curriculum(&state.db, config).await {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }

                    if let Err(e) = state.session.load(&state.db).await {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }
                }

                let previous_scores: HashMap<String, f64> = state
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
                    apply_analysis_to_db(&analysis, &session, &forced_learning_item_ids, &state.db)
                        .await;
                let scores: HashMap<String, f64> = match scores_result {
                    Ok(scores) => scores,
                    Err(e) => {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }
                };

                let evaluated_scores: HashMap<&str, f64> = scores
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
                            < MASTERY_THRESHOLD
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

                let target_topic_name = state
                    .session
                    .topics
                    .iter()
                    .find(|t| Some(&t.id) == state.session.target_topic_id.as_ref())
                    .map(|t| t.name.clone());

                state.report = ReportState::from_analysis(
                    report_analysis,
                    session,
                    weak_topics,
                    state.session.target_topic_id.clone(),
                    target_topic_name,
                );

                session::reset_session(&mut state.session);
                state.view = View::Report;
            }
        }
        Err(e) => {
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

async fn handle_curriculum(state: &mut AppState, res: Result<Curriculum>) {
    let in_session = state.view == View::Session;
    state.curriculum.loading = false;
    if in_session {
        state.session.loading = false;
    }
    state.stream_status = None;
    match res {
        Ok(curriculum) => {
            persist_topics_and_reload(state, &curriculum.topics, true).await;
        }
        Err(e) => {
            if in_session {
                state.session.pending_new_topic = false;
            }
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

async fn handle_curriculum_extension(state: &mut AppState, res: Result<Vec<Topic>>) {
    let in_session = state.view == View::Session;
    state.curriculum.loading = false;
    if in_session {
        state.session.loading = false;
    }
    state.stream_status = None;
    match res {
        Ok(topics) => {
            persist_topics_and_reload(state, &topics, false).await;
        }
        Err(e) => {
            if in_session {
                state.session.pending_new_topic = false;
            }
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

/// Upserts topics into the curriculum (replacing it wholesale when
/// `replace_all` is set), then reloads the curriculum view and — during a
/// session — the session view and any pending new-topic start.
async fn persist_topics_and_reload(state: &mut AppState, topics: &[Topic], replace_all: bool) {
    let in_session = state.view == View::Session;
    let upsert_result: Result<()> = async {
        let table = state.db.curriculum();
        if replace_all {
            table.delete_all().await?;
        }
        for topic in topics {
            table.upsert(topic).await?;
        }
        Ok(())
    }
    .await;
    match upsert_result {
        Ok(()) => {
            if let Err(e) = state.curriculum.load(&state.db).await {
                state.toast = Some(Toast::error(e.to_string()));
            }
            if in_session && let Err(e) = state.session.load(&state.db).await {
                state.toast = Some(Toast::error(e.to_string()));
            }
            if in_session && let Err(e) = session::maybe_start_pending_new_topic(state).await {
                state.toast = Some(Toast::error(e.to_string()));
            }
        }
        Err(e) => {
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

fn handle_topic_review(state: &mut AppState, res: Result<String>) {
    state.docs.loading = false;
    state.stream_status = None;
    match res {
        Ok(text) => {
            state.docs.content = text;
            state.docs.saved = true;
        }
        Err(e) => {
            state.toast = Some(Toast::error(e.to_string()));
        }
    }
}

fn handle_onboarding_models(state: &mut AppState, res: Result<Vec<ModelInfo>>) {
    state.stream_status = None;
    state.onboarding.model_picker.apply_result(res);
    // If only one model, auto-select it for convenience.
    if state.onboarding.model_picker.models.len() == 1 {
        state.onboarding.model = state.onboarding.model_picker.models[0].id.clone();
        state.onboarding.input = state.onboarding.model.clone();
    }
}

fn user_cefr_numeric(config: &OpenCourseConfig) -> i32 {
    cefr_to_numeric(
        config
            .active_profile()
            .self_assessed_cefr
            .as_deref()
            .unwrap_or("beginner"),
    )
    .unwrap_or(1)
}

async fn ensure_topics_exist(
    db: &Database,
    config: &OpenCourseConfig,
    session: &MentorSession,
    data_dir: &Path,
) -> Result<()> {
    let curriculum = db.curriculum().read_all().await?;
    let existing_ids: HashSet<String> = curriculum.topics.iter().map(|t| t.id.clone()).collect();

    let mut missing_ids = HashSet::new();
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

    let client = create_llm_model(config)?;
    let mut progress = db.progress().read_all().await?;
    let user_cefr = user_cefr_numeric(config);

    for topic_id in missing_ids {
        let mut topic = generate_topic_metadata(
            client.as_ref(),
            config.active_profile(),
            &topic_id,
            None,
            Some(data_dir),
        )
        .await?;

        let topic_cefr = topic.cefr_numeric();
        let initial_score = initial_topic_score(topic_cefr, user_cefr);
        topic.order = Some(if initial_score > 0.0 {
            topic_cefr * 1000 - 100
        } else {
            topic_cefr * 1000 + 999
        });

        db.curriculum().upsert(&topic).await?;

        if !progress.topics.iter().any(|p| p.topic_id == topic.id) {
            progress
                .topics
                .push(ProgressTopic::initial(topic.id, initial_score));
        }
    }

    db.progress().write_all(&progress).await?;
    Ok(())
}

async fn ensure_progress_for_curriculum(db: &Database, config: &OpenCourseConfig) -> Result<()> {
    let curriculum = db.curriculum().read_all().await?;
    let mut progress = db.progress().read_all().await?;

    let existing_ids: HashSet<String> =
        progress.topics.iter().map(|t| t.topic_id.clone()).collect();

    let user_cefr = user_cefr_numeric(config);

    for topic in &curriculum.topics {
        if existing_ids.contains(&topic.id) {
            continue;
        }
        let initial_score = initial_topic_score(topic.cefr_numeric(), user_cefr);
        progress
            .topics
            .push(ProgressTopic::initial(topic.id.clone(), initial_score));
    }

    db.progress().write_all(&progress).await?;
    Ok(())
}

async fn ensure_new_topics(db: &Database, new_topics: &[Topic]) -> Result<()> {
    let mut progress = db.progress().read_all().await?;
    let existing_item_ids: HashSet<String> = db
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
            progress
                .topics
                .push(ProgressTopic::initial(topic.id.clone(), 0.0));
        }
    }
    db.progress().write_all(&progress).await?;
    Ok(())
}
