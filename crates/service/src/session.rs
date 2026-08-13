//! Session flow: exercise preparation, session analysis, and applying a
//! finished session's analysis to the database. The caller (CLI adapter)
//! owns the UI state and the task spawning; these functions do the database
//! reads, topic selection, prompt building, and LLM calls.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tokio::sync::mpsc;

use open_course_config::OpenCourseConfig;
use open_course_core::error::{AppError, Result};
use open_course_core::session::{
    AnalysisResult, COMPLETED_THRESHOLD, Exercise, MentorSession, NextSessionTopic,
    pick_next_session_topic, recent_success_rate, select_side_topics, unique_topic_ids,
};
use open_course_core::vocabulary::Lemma;
use open_course_db::Database;
use open_course_db::apply::apply_analysis_to_db;
use open_course_db::curriculum::{Topic, cefr_to_numeric};
use open_course_db::learning_items::{LearningItem, LearningItemsTable, is_learning_item_name};
use open_course_db::lemmas::LemmasTable;
use open_course_db::progress::{ProgressData, ProgressTopic, initial_topic_score};
use open_course_llm::factory::create_llm_model;
use open_course_llm::pipeline::{
    finalize_analysis_with_new_topics, generate_analysis, generate_exercises,
    generate_topic_metadata,
};
use open_course_llm::prompts::{build_batch_analysis_prompt, build_exercise_prompt};
use open_course_llm::result::LlmResult;

use crate::outbox_append;

/// Everything the adapter needs to request a batch of exercises from the LLM
/// and to track the session afterwards.
pub struct ExercisePreparation {
    pub prompt: String,
    pub forced_learning_item_ids: Vec<String>,
    pub forced_lemma_ids: Vec<String>,
}

/// Reads progress, learning items and history from the database, picks side
/// topics and the weakest learning items, and builds the exercise prompt for
/// `target_topic_id`.
pub async fn prepare_exercises(
    db: &Database,
    config: &OpenCourseConfig,
    all_topics: &[Topic],
    target_topic_id: &str,
) -> Result<ExercisePreparation> {
    let profile = config.active_profile();

    let target_topic = all_topics
        .iter()
        .find(|t| t.id == target_topic_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound(format!("Topic {target_topic_id} not found")))?;
    if all_topics.is_empty() {
        return Err(AppError::Config(
            "No topics available. Generate a curriculum first.".to_string(),
        ));
    }
    let progress = db.progress().read_all().await.unwrap_or_default();
    let side_topics = select_side_topics(
        all_topics,
        std::slice::from_ref(&target_topic),
        3,
        &progress,
        chrono::Utc::now(),
    );

    let candidate_topics: Vec<Topic> = std::iter::once(&target_topic)
        .chain(side_topics.iter())
        .cloned()
        .collect();

    let learning_items: Vec<LearningItem> = db
        .learning_items()
        .read_all()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|li| li.target_lang == profile.target_language)
        .collect();
    let forced_learning_items = LearningItemsTable::weakest(&learning_items, 3);
    let forced_learning_item_ids = forced_learning_items
        .iter()
        .map(|li| li.id.clone())
        .collect();

    let lemmas: Vec<Lemma> = db
        .lemmas()
        .read_all()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|l| l.target_lang == profile.target_language)
        .collect();
    // Soft vocabulary frontier: the lowest CEFR level that still has
    // unmastered topics (the same notion as the frontier gate in
    // core::session::topic_selection, which has no public helper). `None`
    // when no level information is available.
    let frontier_cefr = frontier_cefr(all_topics, &progress);
    let forced_vocabulary = LemmasTable::weakest(&lemmas, 5, frontier_cefr);
    let forced_lemma_ids = forced_vocabulary.iter().map(|l| l.id.clone()).collect();

    let history = db.history().read_all().await.unwrap_or_default();
    let success_rate = recent_success_rate(&history, 5);

    let prompt = build_exercise_prompt(
        profile,
        &[target_topic],
        &side_topics,
        &candidate_topics,
        &forced_learning_items,
        &forced_vocabulary,
        config.preferences.batch_size,
        success_rate,
    );

    Ok(ExercisePreparation {
        prompt,
        forced_learning_item_ids,
        forced_lemma_ids,
    })
}

/// Runs the exercise-generation LLM call for a prepared prompt.
pub async fn generate_session_exercises(
    config: &OpenCourseConfig,
    prompt: &str,
    tx: &mpsc::Sender<LlmResult>,
    data_dir: Option<&Path>,
) -> Result<Vec<Exercise>> {
    let model = create_llm_model(config)?;
    generate_exercises(model.as_ref(), prompt, Some(tx), data_dir).await
}

/// Picks what to practice next: a due review topic, a fresh topic, or a
/// signal that the curriculum should be extended first.
pub async fn next_session_topic(db: &Database, topics: &[Topic]) -> Result<NextSessionTopic> {
    let progress = db.progress().read_all().await?;
    Ok(pick_next_session_topic(
        topics,
        &progress,
        chrono::Utc::now(),
    ))
}

/// First topic that has never been practiced, if any.
pub async fn pick_untouched_topic(db: &Database, topics: &[Topic]) -> Result<Option<String>> {
    let progress = db.progress().read_all().await?;
    let touched: HashSet<String> = progress
        .topics
        .iter()
        .filter(|p| p.last_practiced.is_some())
        .map(|p| p.topic_id.clone())
        .collect();

    Ok(topics
        .iter()
        .find(|t| !touched.contains(&t.id))
        .map(|t| t.id.clone()))
}

/// Everything the adapter needs to request the session analysis from the LLM.
pub struct AnalysisPreparation {
    pub prompt: String,
    pub pairs: Vec<(Exercise, String)>,
}

/// Collects the (exercise, answer) pairs of a finished session and builds the
/// batch analysis prompt.
pub fn prepare_analysis(
    config: &OpenCourseConfig,
    session: &MentorSession,
    all_topics: &[Topic],
) -> AnalysisPreparation {
    let profile = config.active_profile();

    let pairs: Vec<(Exercise, String)> = session
        .exercises
        .iter()
        .enumerate()
        .map(|(i, ex)| {
            (
                ex.clone(),
                session.answers.get(&i).cloned().unwrap_or_default(),
            )
        })
        .collect();

    let candidate_ids: HashSet<String> = session
        .exercises
        .iter()
        .flat_map(|ex| ex.target_topic_ids.iter().chain(ex.side_topic_ids.iter()))
        .cloned()
        .collect();
    let candidate_topics: Vec<Topic> = all_topics
        .iter()
        .filter(|t| candidate_ids.contains(&t.id))
        .cloned()
        .collect();

    let prompt = build_batch_analysis_prompt(profile, &pairs, &candidate_topics);

    AnalysisPreparation { prompt, pairs }
}

/// Runs the analysis LLM chain: batch analysis, merging the student's answers
/// back in, then finalizing metadata for any newly reported topics.
pub async fn run_session_analysis(
    config: &OpenCourseConfig,
    all_topics: &[Topic],
    preparation: AnalysisPreparation,
    tx: &mpsc::Sender<LlmResult>,
    data_dir: Option<&Path>,
) -> Result<AnalysisResult> {
    let model = create_llm_model(config)?;
    let mut analysis = generate_analysis(
        model.as_ref(),
        &preparation.prompt,
        preparation.pairs.len(),
        Some(tx),
        data_dir,
    )
    .await?;
    merge_analysis_with_pairs(&mut analysis, &preparation.pairs);
    let analysis = finalize_analysis_with_new_topics(
        model.as_ref(),
        config.active_profile(),
        all_topics,
        analysis,
        Some(tx),
        data_dir,
    )
    .await?;
    Ok(analysis)
}

fn merge_analysis_with_pairs(analysis: &mut AnalysisResult, pairs: &[(Exercise, String)]) {
    for sentence in &mut analysis.sentences {
        let idx = (sentence.sentence_number - 1) as usize;
        if let Some((exercise, answer)) = pairs.get(idx) {
            sentence.student_translation = answer.clone();
            sentence.expected_translation = exercise.expected_translation.clone();
        }
    }
}

/// Neutral outcome of applying a session analysis: the topic masteries before
/// and after the session, for the adapter to present.
pub struct AppliedAnalysis {
    pub previous_scores: HashMap<String, f64>,
    pub scores: HashMap<String, f64>,
}

/// Persists everything a finished session produced: new topics and learning
/// items, metadata for topics the exercises referenced but the curriculum
/// lacks, progress entries for the whole curriculum, then the session scores
/// themselves. When `config` is `None` the ensure-steps are skipped (there is
/// no provider to generate missing topic metadata with).
pub async fn apply_analysis(
    db: &Database,
    config: Option<&OpenCourseConfig>,
    session: &MentorSession,
    analysis: &mut AnalysisResult,
    forced_learning_item_ids: &[String],
    forced_lemma_ids: &[String],
    data_dir: &Path,
) -> Result<AppliedAnalysis> {
    if let Some(config) = config {
        ensure_new_topics(db, &analysis.new_topics).await?;
        ensure_topics_exist(db, config, session, data_dir).await?;
        ensure_progress_for_curriculum(db, config).await?;
    }

    let previous_scores: HashMap<String, f64> = db
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

    let (scores, touched_lemma_ids, touched_form_ids) = apply_analysis_to_db(
        analysis,
        session,
        forced_learning_item_ids,
        forced_lemma_ids,
        db,
    )
    .await?;

    outbox_after_apply(
        db,
        session,
        analysis,
        &scores,
        forced_learning_item_ids,
        &touched_lemma_ids,
        &touched_form_ids,
    )
    .await;

    Ok(AppliedAnalysis {
        previous_scores,
        scores,
    })
}

/// Best-effort outbox entries for everything the session wrote: the session
/// summary, the progress entries it touched, the practiced learning items,
/// and every lemma/form the apply step reports as touched (created,
/// practiced, or CEFR-updated).
async fn outbox_after_apply(
    db: &Database,
    session: &MentorSession,
    analysis: &AnalysisResult,
    scores: &HashMap<String, f64>,
    forced_learning_item_ids: &[String],
    touched_lemma_ids: &[String],
    touched_form_ids: &[String],
) {
    use open_course_db::outbox::{
        ENTITY_FORM, ENTITY_LEARNING_ITEM, ENTITY_LEMMA, ENTITY_PROGRESS, ENTITY_SESSION,
        OP_UPSERT,
    };

    if let Ok(history) = db.history().read_all().await
        && let Some(summary) = history.iter().find(|s| s.id == session.id)
        && let Ok(payload) = serde_json::to_string(summary)
    {
        outbox_append(db, OP_UPSERT, ENTITY_SESSION, &summary.id, &payload).await;
    }

    if let Ok(progress) = db.progress().read_all().await {
        for topic in &progress.topics {
            if scores.contains_key(&topic.topic_id)
                && let Ok(payload) = serde_json::to_string(topic)
            {
                outbox_append(db, OP_UPSERT, ENTITY_PROGRESS, &topic.topic_id, &payload).await;
            }
        }
    }

    let touched_item_ids: HashSet<&str> = forced_learning_item_ids
        .iter()
        .map(String::as_str)
        .chain(analysis.new_learning_items.iter().map(|i| i.id.as_str()))
        .collect();
    if !touched_item_ids.is_empty()
        && let Ok(items) = db.learning_items().read_all().await
    {
        for item in &items {
            if touched_item_ids.contains(item.id.as_str())
                && let Ok(payload) = serde_json::to_string(item)
            {
                outbox_append(db, OP_UPSERT, ENTITY_LEARNING_ITEM, &item.id, &payload).await;
            }
        }
    }

    if !touched_lemma_ids.is_empty()
        && let Ok(lemmas) = db.lemmas().read_all().await
    {
        let touched: HashSet<&str> = touched_lemma_ids.iter().map(String::as_str).collect();
        for lemma in &lemmas {
            if touched.contains(lemma.id.as_str())
                && let Ok(payload) = serde_json::to_string(lemma)
            {
                outbox_append(db, OP_UPSERT, ENTITY_LEMMA, &lemma.id, &payload).await;
            }
        }
    }

    if !touched_form_ids.is_empty()
        && let Ok(forms) = db.forms().read_all().await
    {
        let touched: HashSet<&str> = touched_form_ids.iter().map(String::as_str).collect();
        for form in &forms {
            if touched.contains(form.id.as_str())
                && let Ok(payload) = serde_json::to_string(form)
            {
                outbox_append(db, OP_UPSERT, ENTITY_FORM, &form.id, &payload).await;
            }
        }
    }
}

/// Numeric CEFR (A1=1..C2=6) of the lowest curriculum level that still has
/// unmastered topics (mastery below `COMPLETED_THRESHOLD`; topics without a
/// progress record count as unmastered). `None` when no unfinished topic
/// carries level information.
fn frontier_cefr(topics: &[Topic], progress: &ProgressData) -> Option<i32> {
    let mastery_of = |topic_id: &str| -> f64 {
        progress
            .topics
            .iter()
            .find(|t| t.topic_id == topic_id)
            .map(|t| t.mastery)
            .unwrap_or(0.0)
    };
    topics
        .iter()
        .filter(|t| mastery_of(&t.id) < COMPLETED_THRESHOLD)
        .map(|t| t.cefr_numeric())
        .filter(|n| *n > 0)
        .min()
}

fn user_cefr_numeric(config: &OpenCourseConfig) -> i32 {    cefr_to_numeric(
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
        if let Ok(payload) = serde_json::to_string(&topic) {
            outbox_append(
                db,
                open_course_db::outbox::OP_UPSERT,
                open_course_db::outbox::ENTITY_TOPIC,
                &topic.id,
                &payload,
            )
            .await;
        }

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
                if let Ok(payload) = serde_json::to_string(&item) {
                    outbox_append(
                        db,
                        open_course_db::outbox::OP_UPSERT,
                        open_course_db::outbox::ENTITY_LEARNING_ITEM,
                        &item.id,
                        &payload,
                    )
                    .await;
                }
            }
            continue;
        }
        db.curriculum().upsert(topic).await?;
        if let Ok(payload) = serde_json::to_string(topic) {
            outbox_append(
                db,
                open_course_db::outbox::OP_UPSERT,
                open_course_db::outbox::ENTITY_TOPIC,
                &topic.id,
                &payload,
            )
            .await;
        }
        if !progress.topics.iter().any(|p| p.topic_id == topic.id) {
            progress
                .topics
                .push(ProgressTopic::initial(topic.id.clone(), 0.0));
        }
    }
    db.progress().write_all(&progress).await?;
    Ok(())
}
