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
    #[serde(default)]
    pub acceptable_translations: Vec<String>,
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
    /// Word-specific items (e.g. "Adjective: Caro vs Rico") routed to the
    /// learning_items table instead of the curriculum. Filled by the pipeline,
    /// never by the LLM, so it is excluded from serde and the JSON schema.
    #[serde(skip, default)]
    #[schemars(skip)]
    pub new_learning_items: Vec<crate::db::learning_items::LearningItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SentenceAnalysis {
    pub sentence_number: i32,
    #[serde(default)]
    pub student_translation: String,
    #[serde(default)]
    pub expected_translation: String,
    #[serde(default)]
    pub acceptable_translations: Vec<String>,
    #[serde(default)]
    pub semantic_verdict: SemanticVerdict,
    pub errors: Vec<GrammarError>,
    pub per_sentence_feedback: Vec<FeedbackComment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SemanticVerdict {
    #[default]
    Correct,
    Acceptable,
    NeedsCorrection,
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
    Minor,
    #[default]
    Spelling,
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
    let due = get_due_review_topics(topics, progress, cefr, Utc::now());
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
    now: chrono::DateTime<Utc>,
) -> Vec<Topic> {
    let progress_map: HashMap<_, _> = progress
        .topics
        .iter()
        .map(|t| (&t.topic_id,
            effective_mastery(t, now)
        ))
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

pub fn get_weak_review_topics(topics: &[Topic], progress: &ProgressData, now: chrono::DateTime<Utc>) -> Vec<Topic> {
    let progress_map: HashMap<_, _> = progress.topics.iter().map(|t| (&t.topic_id, t)).collect();

    let mut filtered: Vec<_> = topics
        .iter()
        .filter(|t| match progress_map.get(&t.id) {
            Some(pt) => effective_mastery(pt, now) < 50.0,
            None => false,
        })
        .cloned()
        .collect();

    filtered.sort_by(|a, b| {
        let pt_a = progress_map.get(&a.id);
        let pt_b = progress_map.get(&b.id);

        let practiced_a = pt_a.and_then(|p| p.last_practiced.as_deref());
        let practiced_b = pt_b.and_then(|p| p.last_practiced.as_deref());

        let practiced_cmp = match (practiced_a, practiced_b) {
            (Some(da), Some(db)) => da.cmp(db),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };

        if practiced_cmp != std::cmp::Ordering::Equal {
            return practiced_cmp;
        }

        let score_a = pt_a.map(|p| p.score).unwrap_or(100.0);
        let score_b = pt_b.map(|p| p.score).unwrap_or(100.0);
        score_a
            .partial_cmp(&score_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    filtered
}

/// What the next `n` session should be: a spaced review of a due topic, a
/// fresh curriculum topic, or a signal to extend the curriculum.
#[derive(Debug, Clone, PartialEq)]
pub enum NextSessionTopic {
    Review(Topic),
    New(Topic),
    ExtendCurriculum,
}

/// Backlog size (due topics) at which every second session becomes a review.
const REVIEW_BACKLOG_THRESHOLD: usize = 5;

/// Balances new material against spaced review. Due topics are practiced
/// topics whose effective mastery decayed below 50, weakest first. When both
/// candidates exist, every third session is a review (every second when the
/// backlog is large), so new topics keep flowing at a predictable pace.
pub fn pick_next_session_topic(
    topics: &[Topic],
    progress: &ProgressData,
    now: chrono::DateTime<Utc>,
) -> NextSessionTopic {
    let progress_map: HashMap<&String, &ProgressTopic> =
        progress.topics.iter().map(|t| (&t.topic_id, t)).collect();

    let new_topic = topics
        .iter()
        .find(|t| {
            progress_map
                .get(&t.id)
                .map(|p| p.last_practiced.is_none())
                .unwrap_or(true)
        })
        .cloned();

    let mut due: Vec<(Topic, f64)> = topics
        .iter()
        .filter_map(|t| {
            let p = progress_map.get(&t.id)?;
            p.last_practiced.as_ref()?;
            let mastery = effective_mastery(p, now);
            (mastery < 50.0).then(|| (t.clone(), mastery))
        })
        .collect();
    due.sort_by(|a, b| {
        a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    match (due.first(), new_topic) {
        (None, Some(new)) => NextSessionTopic::New(new),
        (None, None) => NextSessionTopic::ExtendCurriculum,
        (Some((topic, _)), None) => NextSessionTopic::Review(topic.clone()),
        (Some((topic, _)), Some(new)) => {
            let review_every: i32 = if due.len() >= REVIEW_BACKLOG_THRESHOLD {
                2
            } else {
                3
            };
            if (progress.session_count + 1) % review_every == 0 {
                NextSessionTopic::Review(topic.clone())
            } else {
                NextSessionTopic::New(new)
            }
        }
    }
}

const TOPIC_SCORE_ALPHA_BASE: f64 = 0.1;
const TOPIC_SCORE_ALPHA_RANGE: f64 = 0.35;

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
            let relevant_errors: Vec<GrammarError> = sentence
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
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let exercise_score = exercise_score_for_sentence(sentence, &relevant_errors);
            map.entry(topic_id).or_default().push(exercise_score);
        }
    }

    map
}

fn exercise_score_for_sentence(
    sentence: Option<&SentenceAnalysis>,
    relevant_errors: &[GrammarError],
) -> f64 {
    match sentence.map(|s| s.semantic_verdict) {
        Some(SemanticVerdict::Correct) => 100.0,
        Some(SemanticVerdict::Acceptable) => {
            let penalty: f64 = relevant_errors
                .iter()
                .map(|e| match e.error_type {
                    GrammarErrorType::Critical => 10.0,
                    GrammarErrorType::Major => 5.0,
                    GrammarErrorType::Minor => 2.0,
                    GrammarErrorType::Spelling => 0.5,
                })
                .sum();
            (90.0 - penalty).max(70.0)
        }
        _ => error_based_score(relevant_errors.iter()),
    }
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
            GrammarErrorType::Spelling => 1.0,
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

fn adaptive_alpha(mastery: f64) -> f64 {
    let normalized = 1.0 - (mastery / 100.0);
    TOPIC_SCORE_ALPHA_BASE + TOPIC_SCORE_ALPHA_RANGE * normalized.clamp(0.0, 1.0)
}

pub fn recent_success_rate(summaries: &[SessionSummary], n: usize) -> f64 {
    let recent: Vec<_> = summaries.iter().rev().take(n).collect();
    if recent.is_empty() {
        return 0.75;
    }
    let total: f64 = recent.iter().map(|s| s.avg_target_score / 100.0).sum();
    total / recent.len() as f64
}

pub fn average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().sum();
    (sum / values.len() as f64).round()
}

const DECAY_RATE: f64 = 0.05;

pub fn effective_mastery(topic: &ProgressTopic, now: chrono::DateTime<Utc>) -> f64 {
    let days = topic
        .last_practiced
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| (now - dt.with_timezone(&Utc)).num_days() as f64)
        .unwrap_or(0.0)
        .max(0.0);
    let decay_factor = (-DECAY_RATE * days).exp();
    clamp_score((topic.mastery * decay_factor).round())
}

pub fn clamp_score(score: f64) -> f64 {
    score.clamp(0.0, 100.0)
}

pub async fn apply_analysis_to_db(
    analysis: &AnalysisResult,
    session: &MentorSession,
    forced_learning_item_ids: &[String],
    db: &crate::db::Database,
) -> Result<HashMap<String, f64>> {
    let curriculum = db.curriculum().read_all().await?;
    let mut progress = db.progress().read_all().await?;

    let mut learning_items: std::collections::HashMap<String, crate::db::learning_items::LearningItem> = db
        .learning_items()
        .read_all()
        .await?
        .into_iter()
        .map(|li| (li.id.clone(), li))
        .collect();

    // Word-specific items (e.g. "Adjective: Caro vs Rico") are stored as
    // learning items for later review, not as curriculum topics. Entries that
    // already exist keep their accumulated score.
    for item in &analysis.new_learning_items {
        learning_items
            .entry(item.id.clone())
            .or_insert_with(|| item.clone());
    }

    for topic in &analysis.new_topics {
        // Safety net: word-specific names must not become curriculum topics.
        if crate::db::learning_items::is_learning_item_name(&topic.name) {
            let item = crate::db::learning_items::LearningItem::from_topic(topic);
            learning_items.entry(item.id.clone()).or_insert(item);
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

    let now = chrono::Utc::now().to_rfc3339();
    for id in forced_learning_item_ids {
        if let Some(item) = learning_items.get_mut(id) {
            let exercise_score = learning_item_exercise_score(item, analysis, session);
            item.score = ema_update(item.score, exercise_score);
            item.last_practiced = Some(now.clone());
            item.practice_count += 1;
        }
    }

    for item in learning_items.values() {
        db.learning_items().upsert(item).await?;
    }

    let history = db.history();
    let scores = apply_analysis(analysis, session, &curriculum, &mut progress, &history).await?;
    db.progress().write_all(&progress).await?;
    Ok(scores)
}

fn ema_update(base: f64, session_score: f64) -> f64 {
    let alpha = 0.12;
    let raw = base * (1.0 - alpha) + session_score * alpha;
    raw.round().clamp(0.0, 100.0)
}

fn learning_item_exercise_score(
    item: &crate::db::learning_items::LearningItem,
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
