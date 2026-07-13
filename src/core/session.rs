use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::db::curriculum::{Curriculum, Topic};
use crate::db::history::{HistoryTable, SessionSummary};
use crate::db::progress::{ProgressData, ProgressTopic};
use crate::error::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct MentorSession {
    pub id: String,
    pub exercises: Vec<Exercise>,
    pub answers: HashMap<usize, String>,
    pub current_exercise_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Exercise {
    pub id: String,
    pub target_sentence: String,
    pub expected_translation: String,
    pub target_topic_ids: Vec<String>,
    pub side_topic_ids: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec_string")]
    pub expected_patterns: Vec<String>,
    pub hint: Option<String>,
}

fn string_or_vec_string<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrVec;

    impl<'de> serde::de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or an array of strings")
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, seq: S) -> std::result::Result<Self::Value, S::Error>
        where
            S: serde::de::SeqAccess<'de>,
        {
            serde::Deserialize::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub session_score: Option<f64>,
    #[serde(default)]
    pub sentences: Vec<SentenceAnalysis>,
    #[serde(default)]
    pub evaluated_topics: Vec<EvaluatedTopic>,
    #[serde(default)]
    pub new_topics: Vec<Topic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SentenceAnalysis {
    pub sentence_number: i32,
    #[serde(default)]
    pub student_translation: String,
    #[serde(default)]
    pub expected_translation: String,
    pub errors: Vec<GrammarError>,
    pub per_sentence_feedback: Vec<FeedbackComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackComment {
    pub comment: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GrammarError {
    #[serde(rename = "type", default)]
    pub error_type: GrammarErrorType,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub explanation: String,
    #[serde(default)]
    pub topic_ids: Vec<String>,
    #[serde(default)]
    pub new_topics: Vec<NewTopicRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewTopicRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub level: Option<String>,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum GrammarErrorType {
    Critical,
    Major,
    #[default]
    Minor,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedTopic {
    pub topic_id: String,
    pub score: f64,
    #[serde(default)]
    pub previous_score: Option<f64>,
}

pub fn create_session(exercises: Vec<Exercise>, batch_size: usize) -> MentorSession {
    MentorSession {
        id: format!("{}", Utc::now().timestamp_millis()),
        exercises: exercises.into_iter().take(batch_size).collect(),
        answers: HashMap::new(),
        current_exercise_index: 0,
    }
}

pub fn record_answer(session: &MentorSession, index: usize, answer: String) -> MentorSession {
    let mut updated = session.clone();
    updated.answers.insert(index, answer);
    updated
}

pub fn advance_exercise(session: &MentorSession) -> MentorSession {
    let mut updated = session.clone();
    updated.current_exercise_index += 1;
    updated
}

pub fn is_session_complete(session: &MentorSession) -> bool {
    session.current_exercise_index >= session.exercises.len()
}

pub fn select_target_topics(
    topics: &[Topic],
    progress: &ProgressData,
    cefr: Option<&str>,
) -> Vec<Topic> {
    if topics.is_empty() {
        return Vec::new();
    }
    let due = get_due_review_topics(topics, progress, cefr);
    due.into_iter().take(1).collect()
}

pub fn select_side_topics(topics: &[Topic], exclude: &[Topic], count: usize) -> Vec<Topic> {
    if topics.is_empty() {
        return Vec::new();
    }
    let exclude_ids: std::collections::HashSet<_> = exclude.iter().map(|t| &t.id).collect();
    topics
        .iter()
        .filter(|t| !exclude_ids.contains(&t.id))
        .take(count)
        .cloned()
        .collect()
}

pub fn get_due_review_topics(
    topics: &[Topic],
    progress: &ProgressData,
    cefr: Option<&str>,
) -> Vec<Topic> {
    let progress_map: HashMap<_, _> = progress
        .topics
        .iter()
        .map(|t| (&t.topic_id, t.score))
        .collect();

    let mut filtered: Vec<_> = topics
        .iter()
        .filter(|t| match progress_map.get(&t.id) {
            Some(score) => *score < 50.0,
            None => true,
        })
        .cloned()
        .collect();

    filtered.sort_by(|a, b| {
        let score_a = progress_map.get(&a.id).copied().unwrap_or(0.0);
        let score_b = progress_map.get(&b.id).copied().unwrap_or(0.0);
        score_a
            .partial_cmp(&score_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some(cefr) = cefr {
        let cefr_order = ["A1", "A2", "B1", "B2", "C1", "C2"];
        let user_index = cefr_order.iter().position(|c| c == &cefr.to_uppercase());
        if let Some(user_index) = user_index {
            let difficulty_map: HashMap<_, _> =
                [("beginner", 1), ("intermediate", 3), ("advanced", 5)]
                    .into_iter()
                    .collect();
            let mut with_distance: Vec<_> = filtered
                .into_iter()
                .map(|t| {
                    let diff = difficulty_map
                        .get(t.difficulty.as_str())
                        .copied()
                        .unwrap_or(3);
                    (t, (diff - user_index as i32).abs())
                })
                .collect();
            with_distance.sort_by_key(|a| a.1);
            filtered = with_distance.into_iter().map(|(t, _)| t).collect();
        }
    }

    filtered
}

pub fn get_weak_review_topics(topics: &[Topic], progress: &ProgressData) -> Vec<Topic> {
    let progress_map: HashMap<_, _> = progress
        .topics
        .iter()
        .map(|t| (&t.topic_id, t.score))
        .collect();

    let mut filtered: Vec<_> = topics
        .iter()
        .filter(|t| match progress_map.get(&t.id) {
            Some(score) => *score < 50.0,
            None => false,
        })
        .cloned()
        .collect();

    filtered.sort_by(|a, b| {
        let score_a = progress_map.get(&a.id).copied().unwrap_or(100.0);
        let score_b = progress_map.get(&b.id).copied().unwrap_or(100.0);
        score_a
            .partial_cmp(&score_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    filtered
}

const TOPIC_SCORE_ALPHA: f64 = 0.12;

pub async fn apply_analysis(
    analysis: &AnalysisResult,
    session: &MentorSession,
    _curriculum: &Curriculum,
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
        let base = existing.map(|t| t.score).unwrap_or(0.0);
        let mut score = base;
        for exercise_score in scores {
            let previous = score;
            score = score * (1.0 - TOPIC_SCORE_ALPHA) + exercise_score * TOPIC_SCORE_ALPHA;
            if exercise_score < 100.0 && score > previous {
                score = previous;
            }
        }
        score = clamp_score(score.round());

        final_scores.insert(topic_id.clone(), score);

        let updated = ProgressTopic {
            topic_id: topic_id.clone(),
            score,
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
    if avg_target_score < 60.0 {
        alerts.push("low_session_score".to_string());
    }
    if analysis.sentences.iter().any(|s| !s.errors.is_empty()) {
        alerts.push("review_session_errors".to_string());
    }
    if progress_data.topics.iter().any(|t| t.score < 50.0) {
        alerts.push("focus_on_weak_topics".to_string());
    }
    progress_data.adaptive_alerts.extend(alerts);
    progress_data.adaptive_alerts.sort();
    progress_data.adaptive_alerts.dedup();

    Ok(final_scores)
}

fn topic_exercise_scores(
    session: &MentorSession,
    analysis: &AnalysisResult,
) -> HashMap<String, Vec<f64>> {
    let mut map: HashMap<String, Vec<f64>> = HashMap::new();
    let sentence_by_number: HashMap<i32, &SentenceAnalysis> = analysis
        .sentences
        .iter()
        .map(|s| (s.sentence_number, s))
        .collect();

    for (i, exercise) in session.exercises.iter().enumerate() {
        let sentence_number = (i + 1) as i32;
        let sentence = sentence_by_number.get(&sentence_number).copied();

        let exercise_topic_ids = unique_topic_ids(
            exercise
                .target_topic_ids
                .iter()
                .chain(exercise.side_topic_ids.iter())
                .cloned(),
        );

        let mut topic_ids = exercise_topic_ids.clone();
        if let Some(s) = sentence {
            for error in &s.errors {
                topic_ids.extend(error.topic_ids.iter().cloned());
            }
        }
        topic_ids = unique_topic_ids(topic_ids);

        for topic_id in topic_ids {
            let relevant_errors: Vec<&GrammarError> = sentence
                .map(|s| {
                    s.errors
                        .iter()
                        .filter(|e| {
                            if e.topic_ids.is_empty() {
                                exercise_topic_ids.contains(&topic_id)
                            } else {
                                e.topic_ids.contains(&topic_id)
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let exercise_score = if relevant_errors.is_empty() {
                100.0
            } else {
                error_based_score(relevant_errors)
            };
            map.entry(topic_id).or_default().push(exercise_score);
        }
    }

    map
}

pub fn collect_topic_errors(
    session: &MentorSession,
    analysis: &AnalysisResult,
) -> HashMap<String, Vec<GrammarError>> {
    let mut map: HashMap<String, Vec<GrammarError>> = HashMap::new();

    let mut sentence_by_number: HashMap<i32, &SentenceAnalysis> = HashMap::new();
    for sentence in &analysis.sentences {
        sentence_by_number.insert(sentence.sentence_number, sentence);
    }

    for (i, exercise) in session.exercises.iter().enumerate() {
        let sentence_number = (i + 1) as i32;
        let sentence = match sentence_by_number.get(&sentence_number) {
            Some(s) => s,
            None => continue,
        };
        let topic_ids = unique_topic_ids(
            exercise
                .target_topic_ids
                .iter()
                .chain(exercise.side_topic_ids.iter())
                .cloned(),
        );
        for error in &sentence.errors {
            for topic_id in &topic_ids {
                map.entry(topic_id.clone()).or_default().push(error.clone());
            }
        }
    }
    map
}

pub fn count_topic_occurrences(session: &MentorSession) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for exercise in &session.exercises {
        let ids = unique_topic_ids(
            exercise
                .target_topic_ids
                .iter()
                .chain(exercise.side_topic_ids.iter())
                .cloned(),
        );
        for id in ids {
            *counts.entry(id).or_default() += 1;
        }
    }
    counts
}

pub fn unique_topic_ids<I>(ids: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashMap::new();
    for id in ids {
        seen.insert(id, ());
    }
    seen.into_keys().collect()
}

pub fn error_penalty<'a, I>(errors: I) -> f64
where
    I: IntoIterator<Item = &'a GrammarError>,
{
    errors
        .into_iter()
        .map(|e| match e.error_type {
            GrammarErrorType::Critical => 15.0,
            GrammarErrorType::Major => 8.0,
            GrammarErrorType::Minor => 3.0,
        })
        .sum::<f64>()
        .min(40.0)
}

pub fn error_based_score<'a, I>(errors: I) -> f64
where
    I: IntoIterator<Item = &'a GrammarError>,
{
    let errors: Vec<_> = errors.into_iter().collect();
    if errors.is_empty() {
        100.0
    } else {
        (50.0 - error_penalty(errors)).max(0.0)
    }
}

pub fn weighted_ema_score(base_score: f64, session_score: f64, occurrences: usize) -> f64 {
    let weight = occurrences as f64 / (occurrences as f64 + 3.0);
    let raw = base_score * (1.0 - weight) + session_score * weight;
    raw.round().clamp(0.0, 100.0)
}

pub fn average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().sum();
    (sum / values.len() as f64).round()
}

pub fn clamp_score(score: f64) -> f64 {
    score.clamp(0.0, 100.0)
}

pub async fn apply_analysis_to_db(
    analysis: &AnalysisResult,
    session: &MentorSession,
    db: &crate::db::Database,
) -> Result<HashMap<String, f64>> {
    let curriculum = db.curriculum().read_all().await?;
    let mut progress = db.progress().read_all().await?;
    for topic in &analysis.new_topics {
        db.curriculum().upsert(topic).await?;
        if !progress.topics.iter().any(|p| p.topic_id == topic.id) {
            progress.topics.push(ProgressTopic {
                topic_id: topic.id.clone(),
                score: 0.0,
                last_practiced: None,
            });
        }
    }
    let history = db.history();
    let scores = apply_analysis(analysis, session, &curriculum, &mut progress, &history).await?;
    db.progress().write_all(&progress).await?;
    Ok(scores)
}
