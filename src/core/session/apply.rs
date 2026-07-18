//! Applying a finished session's analysis to the database: topic mastery
//! updates, history records, adaptive alerts and learning items.

use std::collections::HashMap;

use chrono::Utc;

use crate::db::curriculum::Topic;
use crate::db::history::{HistoryTable, SessionSummary};
use crate::db::learning_items::{
    LearningItem, is_duplicate_name, is_learning_item_name, significant_words,
    text_contains_any_word,
};
use crate::db::progress::{ProgressData, ProgressTopic};
use crate::error::Result;

use super::models::AnalysisResult;
use super::scoring::{adaptive_alpha, average, clamp_score, ema_update, topic_exercise_scores};
use super::{LOW_SESSION_SCORE_THRESHOLD, MASTERY_THRESHOLD, MentorSession, unique_topic_ids};

pub async fn apply_analysis(
    analysis: &AnalysisResult,
    session: &MentorSession,
    progress_data: &mut ProgressData,
    history_table: &HistoryTable,
) -> Result<HashMap<String, f64>> {
    let target_ids = unique_topic_ids(
        session
            .exercises
            .iter()
            .flat_map(|e| e.target_topic_ids.iter().cloned()),
    );
    let side_ids = unique_topic_ids(
        session
            .exercises
            .iter()
            .flat_map(|e| e.side_topic_ids.iter().cloned()),
    );
    let session_topic_ids = unique_topic_ids(
        target_ids
            .iter()
            .cloned()
            .chain(side_ids.iter().cloned())
            .chain(
                analysis
                    .sentences
                    .iter()
                    .flat_map(|s| s.errors.iter().flat_map(|e| e.topic_ids.iter().cloned())),
            ),
    );

    let exercise_scores_by_topic = topic_exercise_scores(session, analysis);

    let mut final_scores = HashMap::new();
    let now = Utc::now().to_rfc3339();

    for topic_id in &session_topic_ids {
        let scores = exercise_scores_by_topic
            .get(topic_id)
            .cloned()
            .unwrap_or_default();
        let existing = progress_data
            .topics
            .iter()
            .find(|t| &t.topic_id == topic_id);
        let base_mastery = existing.map(|t| t.mastery).unwrap_or(0.0);
        let mut mastery = base_mastery;
        let mut practice_count = existing.map(|t| t.practice_count).unwrap_or(0);
        for exercise_score in scores {
            let alpha = adaptive_alpha(mastery);
            mastery = mastery * (1.0 - alpha) + exercise_score * alpha;
            practice_count += 1;
        }
        mastery = clamp_score(mastery.round());

        final_scores.insert(topic_id.clone(), mastery);

        let updated = ProgressTopic {
            topic_id: topic_id.clone(),
            score: mastery,
            mastery,
            difficulty_estimate: existing.map(|t| t.difficulty_estimate).unwrap_or(0.0),
            practice_count,
            last_practiced: Some(now.clone()),
        };

        if let Some(pos) = progress_data
            .topics
            .iter()
            .position(|t| &t.topic_id == topic_id)
        {
            progress_data.topics[pos] = updated;
        } else {
            progress_data.topics.push(updated);
        }
    }

    let target_scores: Vec<f64> = target_ids
        .iter()
        .map(|id| *final_scores.get(id).unwrap_or(&0.0))
        .collect();
    let avg_target_score = average(&target_scores);

    let summary = SessionSummary {
        id: session.id.clone(),
        date: now,
        target_topic_ids: target_ids,
        side_topic_ids: side_ids,
        new_topic_ids: analysis.new_topics.iter().map(|t| t.id.clone()).collect(),
        avg_target_score,
        target_delta: 0.0,
    };

    history_table.append(&summary).await?;

    progress_data.session_count += 1;

    let mut alerts = Vec::new();
    if avg_target_score < LOW_SESSION_SCORE_THRESHOLD {
        alerts.push("low_session_score".to_string());
    }
    if analysis.sentences.iter().any(|s| !s.errors.is_empty()) {
        alerts.push("review_session_errors".to_string());
    }
    if progress_data
        .topics
        .iter()
        .any(|t| t.score < MASTERY_THRESHOLD)
    {
        alerts.push("focus_on_weak_topics".to_string());
    }
    progress_data.adaptive_alerts.extend(alerts);
    progress_data.adaptive_alerts.sort();
    progress_data.adaptive_alerts.dedup();

    Ok(final_scores)
}

pub async fn apply_analysis_to_db(
    analysis: &AnalysisResult,
    session: &MentorSession,
    forced_learning_item_ids: &[String],
    db: &crate::db::Database,
) -> Result<HashMap<String, f64>> {
    let mut progress = db.progress().read_all().await?;

    let mut learning_items: HashMap<String, LearningItem> = db
        .learning_items()
        .read_all()
        .await?
        .into_iter()
        .map(|li| (li.id.clone(), li))
        .collect();

    // (id, name) of every known learning item and curriculum topic, sorted
    // for deterministic fuzzy-dedup matching below.
    let mut known_items: Vec<(String, String)> = learning_items
        .values()
        .map(|li| (li.id.clone(), li.name.clone()))
        .collect();
    known_items.sort();
    let existing_curriculum = db.curriculum().read_all().await?;
    let mut known_topics: Vec<(String, String)> = existing_curriculum
        .topics
        .iter()
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect();
    known_topics.sort();

    // Items that get practice credit this session: the forced ones plus any
    // existing item a new entry was deduplicated into.
    let mut practiced_item_ids: Vec<String> = forced_learning_item_ids.to_vec();

    // Word-specific items (e.g. "Adjective: Caro vs Rico") are stored as
    // learning items for later review, not as curriculum topics. Entries that
    // already exist keep their accumulated score; fuzzy duplicates are merged
    // into the existing item instead of being created.
    for item in &analysis.new_learning_items {
        insert_learning_item(
            &mut learning_items,
            &mut known_items,
            &mut practiced_item_ids,
            item.clone(),
        );
    }

    for topic in &analysis.new_topics {
        // Safety net: word-specific names must not become curriculum topics.
        if is_learning_item_name(&topic.name) {
            let item = LearningItem::from_topic(topic);
            insert_learning_item(
                &mut learning_items,
                &mut known_items,
                &mut practiced_item_ids,
                item,
            );
            continue;
        }
        let topic_id = match find_duplicate_topic(&known_topics, topic) {
            Some(existing_id) => existing_id,
            None => {
                db.curriculum().upsert(topic).await?;
                match known_topics.iter_mut().find(|(id, _)| id == &topic.id) {
                    Some(entry) => entry.1 = topic.name.clone(),
                    None => known_topics.push((topic.id.clone(), topic.name.clone())),
                }
                topic.id.clone()
            }
        };
        if !progress.topics.iter().any(|p| p.topic_id == topic_id) {
            progress
                .topics
                .push(ProgressTopic::initial(topic_id, 0.0));
        }
    }

    let now = Utc::now().to_rfc3339();
    for id in &practiced_item_ids {
        if let Some(item) = learning_items.get_mut(id) {
            // Items that never occurred in the session keep their score and
            // practice stats untouched.
            if let Some(exercise_score) = learning_item_exercise_score(item, analysis, session) {
                item.score = ema_update(item.score, exercise_score);
                item.last_practiced = Some(now.clone());
                item.practice_count += 1;
            }
        }
    }

    for item in learning_items.values() {
        db.learning_items().upsert(item).await?;
    }

    let history = db.history();
    let scores = apply_analysis(analysis, session, &mut progress, &history).await?;
    db.progress().write_all(&progress).await?;
    Ok(scores)
}

/// Inserts a new learning item, merging fuzzy name duplicates into the
/// existing entry: the duplicate is not created and the existing item is
/// scheduled for practice credit this session (appended to `practiced_ids`).
fn insert_learning_item(
    learning_items: &mut HashMap<String, LearningItem>,
    known_items: &mut Vec<(String, String)>,
    practiced_ids: &mut Vec<String>,
    item: LearningItem,
) {
    if learning_items.contains_key(&item.id) {
        return;
    }
    let names: Vec<String> = known_items.iter().map(|(_, name)| name.clone()).collect();
    if let Some(pos) = is_duplicate_name(&names, &item.name) {
        let existing_id = known_items[pos].0.clone();
        if !practiced_ids.contains(&existing_id) {
            practiced_ids.push(existing_id);
        }
        return;
    }
    known_items.push((item.id.clone(), item.name.clone()));
    learning_items.insert(item.id.clone(), item);
}

/// Returns the id of an existing curriculum topic whose name is a fuzzy
/// duplicate of `topic`'s name, or None when the topic is genuinely new.
/// A matching id is an update of the same topic, not a duplicate.
fn find_duplicate_topic(known_topics: &[(String, String)], topic: &Topic) -> Option<String> {
    if known_topics.iter().any(|(id, _)| id == &topic.id) {
        return None;
    }
    let names: Vec<String> = known_topics.iter().map(|(_, name)| name.clone()).collect();
    is_duplicate_name(&names, &topic.name).map(|pos| known_topics[pos].0.clone())
}

/// Scores how the session practiced a learning item: 0 when an error
/// mentions it, 100 when it occurred without errors, and None when the item
/// did not occur in the session at all (the caller then leaves its score and
/// practice stats untouched).
///
/// Occurrence is matched on whole normalized significant words of the item
/// name (falling back to the description, then to the legacy full-name
/// substring match when neither yields any words) against the exercise
/// target sentence and the expected/student translations.
fn learning_item_exercise_score(
    item: &LearningItem,
    analysis: &AnalysisResult,
    session: &MentorSession,
) -> Option<f64> {
    let mut key_words = significant_words(&item.name);
    if key_words.is_empty() {
        key_words = significant_words(&item.description);
    }
    if key_words.is_empty() {
        // No usable words at all: keep the legacy full-name matching.
        return Some(legacy_item_exercise_score(item, analysis, session));
    }

    let mut encountered = false;
    let mut mentioned_in_error = false;
    for (i, exercise) in session.exercises.iter().enumerate() {
        let sentence_number = (i + 1) as i32;
        let sentence = analysis
            .sentences
            .iter()
            .find(|s| s.sentence_number == sentence_number);

        if text_contains_any_word(&exercise.target_sentence, &key_words) {
            encountered = true;
        }
        if let Some(s) = sentence {
            if text_contains_any_word(&s.expected_translation, &key_words)
                || text_contains_any_word(&s.student_translation, &key_words)
            {
                encountered = true;
            }
            for error in &s.errors {
                if text_contains_any_word(&error.pattern, &key_words)
                    || text_contains_any_word(&error.explanation, &key_words)
                    || error
                        .new_topics
                        .iter()
                        .any(|nt| text_contains_any_word(&nt.name, &key_words))
                {
                    mentioned_in_error = true;
                }
            }
        }
    }

    if !encountered {
        return None;
    }
    Some(if mentioned_in_error { 0.0 } else { 100.0 })
}

/// Pre-word-matching behaviour: the full lowercased item name must appear
/// in the error text. Used only when neither the item's name nor its
/// description yields any significant words.
fn legacy_item_exercise_score(
    item: &LearningItem,
    analysis: &AnalysisResult,
    session: &MentorSession,
) -> f64 {
    let item_name_lower = item.name.to_lowercase();
    for (i, _exercise) in session.exercises.iter().enumerate() {
        let sentence_number = (i + 1) as i32;
        let sentence = analysis
            .sentences
            .iter()
            .find(|s| s.sentence_number == sentence_number);
        if let Some(s) = sentence {
            for error in &s.errors {
                let pattern_lower = error.pattern.to_lowercase();
                let explanation_lower = error.explanation.to_lowercase();
                if pattern_lower.contains(&item_name_lower)
                    || explanation_lower.contains(&item_name_lower)
                    || error.new_topics.iter().any(|nt| {
                        nt.name.to_lowercase().contains(&item_name_lower)
                    })
                {
                    return 0.0;
                }
            }
        }
    }
    100.0
}
