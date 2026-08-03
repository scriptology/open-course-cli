//! Handlers for the results of background LLM tasks, one function per
//! `LlmResult` variant. `apply_llm_result` is the thin dispatcher called by
//! the event loop.

use std::collections::HashMap;

use crate::app::{AppState, LlmResult, View, clear_loading};
use crate::ui::views::{ReportState, session};
use crate::ui::widgets::Toast;
use open_course_core::error::Result;
use open_course_core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, MASTERY_THRESHOLD, create_session,
};
use open_course_db::curriculum::{Curriculum, Topic};
use open_course_llm::diagnostics::CheckResult;
use open_course_llm::model_listing::ModelInfo;
use open_course_llm::pipeline::log_debug_event;

pub async fn apply_llm_result(state: &mut AppState, result: LlmResult) {
    if let Some((tag, message)) = debug_describe(&result) {
        log_debug_event(tag, &message, Some(state.data_dir.as_path()));
    }

    // The update check is UI-independent: it must apply even when a previous
    // background task was cancelled.
    if let LlmResult::UpdateCheck(latest) = result {
        handle_update_check(state, latest);
        return;
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
        // Handled above, before the cancellation check.
        LlmResult::UpdateCheck(_) => {}
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
        LlmResult::UpdateCheck(latest) => (
            "update",
            format!("apply_llm_result UpdateCheck: {latest:?}"),
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

/// Store the latest release version and, when the user is on a top-level
/// screen, show the update prompt. During a session or on other focused
/// screens only the dashboard badge is armed.
fn handle_update_check(state: &mut AppState, latest: Option<String>) {
    let Some(latest) = latest else {
        return;
    };
    if !crate::update::is_newer(crate::update::CURRENT_VERSION, &latest) {
        return;
    }
    state.update.latest_version = Some(latest);
    if matches!(state.view, View::Dashboard | View::Onboarding) {
        state.view = View::UpdateAvailable;
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
                let forced_learning_item_ids = state.session.learning_item_ids.clone();
                let applied = match open_course_service::session::apply_analysis(
                    &state.db,
                    state.config.as_ref(),
                    &session,
                    &analysis,
                    &forced_learning_item_ids,
                    &state.data_dir,
                )
                .await
                {
                    Ok(applied) => applied,
                    Err(e) => {
                        state.toast = Some(Toast::error(e.to_string()));
                        return;
                    }
                };

                if state.config.is_some()
                    && let Err(e) = state.session.load(&state.db).await
                {
                    state.toast = Some(Toast::error(e.to_string()));
                    return;
                }

                let previous_scores = applied.previous_scores;
                let scores: HashMap<String, f64> = applied.scores;

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

                // Push the session's changes in the background when sync is
                // enabled for this pair; failures only surface in the
                // Account section, the outbox is kept.
                crate::app::sync::schedule(state, crate::app::sync::SyncTrigger::AfterAnalysis)
                    .await;
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
    match open_course_service::curriculum::persist_topics(&state.db, topics, replace_all).await {
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
            // A freshly (re)generated curriculum is synced data: push it.
            crate::app::sync::schedule(state, crate::app::sync::SyncTrigger::DataChanged).await;
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
