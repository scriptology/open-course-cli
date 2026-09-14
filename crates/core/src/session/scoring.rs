//! Score computation for sessions, topics and learning items.
//!
//! This module intentionally hosts two parallel EMA systems and two parallel
//! error-penalty scales — they measure different things and must NOT be
//! unified:
//!
//! - Topic mastery (session-level): `adaptive_alpha` with alpha in 0.1..=0.45
//!   depending on current mastery, fed by `topic_exercise_scores`. Module
//!   unit ids (`unit_` namespace, from `Exercise::target_unit_ids`) are
//!   scored through the same map and the same EMA, sharing the generic
//!   `progress` rows with grammar topics.
//! - Learning items (item-level): `ema_update` with a fixed alpha of 0.34.
//!
//! - "Acceptable" sentences: per-exercise penalties of 10/5/2/0.5 inside
//!   `exercise_score_for_sentence`, keeping the score in the 70..=90 band.
//! - Wrong sentences: `error_penalty` with 15/8/3/1 (capped at 40) inside
//!   `error_based_score`, starting from a base of 50.

use std::collections::HashMap;

use crate::history::SessionSummary;

use super::models::{
    AnalysisResult, GrammarError, GrammarErrorType, SemanticVerdict, SentenceAnalysis,
};
use super::{MentorSession, unique_topic_ids};

pub fn topic_exercise_scores(
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
        let exercise_unit_ids =
            unique_topic_ids(exercise.target_unit_ids.iter().flatten().cloned());

        let mut topic_ids = exercise_topic_ids.clone();
        topic_ids.extend(exercise_unit_ids.iter().cloned());
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
                            if exercise_unit_ids.contains(&topic_id) {
                                // A unit is measured by the exercise as a
                                // whole, regardless of which grammar topic an
                                // error was attributed to.
                                true
                            } else if e.topic_ids.is_empty() {
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
            // Acceptable-answer penalties (10/5/2/0.5) are deliberately
            // gentler than the `error_penalty` scale used for wrong answers:
            // the score stays in the 70..=90 band.
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

/// Penalty scale for wrong sentences (critical/major/minor/spelling =
/// 15/8/3/1, capped at 40). Deliberately harsher than the acceptable-answer
/// scale inside `exercise_score_for_sentence` (10/5/2/0.5) — different
/// scales for different verdicts, do not unify.
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

const TOPIC_SCORE_ALPHA_BASE: f64 = 0.1;
const TOPIC_SCORE_ALPHA_RANGE: f64 = 0.35;

/// Topic-mastery EMA (session-level): alpha adapts to the current mastery,
/// from 0.45 at 0 down to 0.1 at 100, so weak topics move fast and strong
/// ones stay stable. Parallel to `ema_update` for learning items, but on an
/// independent scale — do not unify.
pub fn adaptive_alpha(mastery: f64) -> f64 {
    let normalized = 1.0 - (mastery / 100.0);
    TOPIC_SCORE_ALPHA_BASE + TOPIC_SCORE_ALPHA_RANGE * normalized.clamp(0.0, 1.0)
}

/// Learning-item EMA (item-level): fixed alpha of 0.34, so a clean session
/// moves a weak item from 0 to 34 and a second one past the mastery
/// threshold of 50. Parallel to the topic-mastery EMA (`adaptive_alpha`),
/// but on an independent scale — do not unify.
pub fn ema_update(base: f64, session_score: f64) -> f64 {
    let alpha = 0.34;
    let raw = base * (1.0 - alpha) + session_score * alpha;
    raw.round().clamp(0.0, 100.0)
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

pub fn clamp_score(score: f64) -> f64 {
    score.clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::models::Exercise;

    fn exercise(target_topic_ids: &[&str], target_unit_ids: Option<&[&str]>) -> Exercise {
        Exercise {
            id: "e1".to_string(),
            target_sentence: "Hello".to_string(),
            expected_translation: "Привет".to_string(),
            acceptable_translations: vec![],
            target_topic_ids: target_topic_ids.iter().map(|s| s.to_string()).collect(),
            side_topic_ids: vec![],
            target_unit_ids: target_unit_ids.map(|ids| ids.iter().map(|s| s.to_string()).collect()),
            expected_patterns: vec![],
            hint: None,
        }
    }

    fn error(error_type: GrammarErrorType, topic_ids: &[&str]) -> GrammarError {
        GrammarError {
            error_type,
            topic_ids: topic_ids.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn session_with(exercises: Vec<Exercise>) -> MentorSession {
        MentorSession {
            id: "s1".to_string(),
            exercises,
            answers: HashMap::new(),
            current_exercise_index: 0,
        }
    }

    fn analysis_with(sentences: Vec<SentenceAnalysis>) -> AnalysisResult {
        AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
            new_lemmas: vec![],
            new_forms: vec![],
        }
    }

    fn sentence(
        number: i32,
        verdict: SemanticVerdict,
        errors: Vec<GrammarError>,
    ) -> SentenceAnalysis {
        SentenceAnalysis {
            sentence_number: number,
            student_translation: String::new(),
            expected_translation: String::new(),
            acceptable_translations: vec![],
            semantic_verdict: verdict,
            errors,
            per_sentence_feedback: vec![],
            used_vocabulary: vec![],
        }
    }

    fn wrong_sentence(number: i32, errors: Vec<GrammarError>) -> SentenceAnalysis {
        sentence(number, SemanticVerdict::NeedsCorrection, errors)
    }

    #[test]
    fn unit_ids_are_scored_from_the_whole_exercise() {
        // One exercise practicing grammar topic g1 and unit unit_1; the wrong
        // sentence has a major error attributed to g1 and a minor one
        // attributed to g2 (not practiced by the exercise).
        let session = session_with(vec![exercise(&["g1"], Some(&["unit_1"]))]);
        let analysis = analysis_with(vec![wrong_sentence(
            1,
            vec![
                error(GrammarErrorType::Major, &["g1"]),
                error(GrammarErrorType::Minor, &["g2"]),
            ],
        )]);

        let scores = topic_exercise_scores(&session, &analysis);

        // Grammar topics only see errors attributed to them: 50-8 and 50-3.
        assert_eq!(scores["g1"], vec![42.0]);
        assert_eq!(scores["g2"], vec![47.0]);
        // The unit is measured by the exercise as a whole: 50-8-3.
        assert_eq!(scores["unit_1"], vec![39.0]);
    }

    #[test]
    fn unit_ema_shares_the_topic_alpha_schedule() {
        // A clean correct sentence scores the unit 100 like any topic.
        let session = session_with(vec![exercise(&[], Some(&["unit_1"]))]);
        let analysis = analysis_with(vec![sentence(1, SemanticVerdict::Correct, vec![])]);
        let scores = topic_exercise_scores(&session, &analysis);
        assert_eq!(scores["unit_1"], vec![100.0]);

        // One EMA step from mastery 0: alpha is 0.45, as for grammar topics.
        let mastery = clamp_score(
            (0.0 * (1.0 - adaptive_alpha(0.0)) + scores["unit_1"][0] * adaptive_alpha(0.0)).round(),
        );
        assert_eq!(mastery, 45.0);
    }

    #[test]
    fn exercises_without_unit_ids_score_grammar_only() {
        let session = session_with(vec![exercise(&["g1"], None)]);
        let analysis = analysis_with(vec![wrong_sentence(
            1,
            vec![error(GrammarErrorType::Major, &["g1"])],
        )]);
        let scores = topic_exercise_scores(&session, &analysis);
        assert_eq!(scores.len(), 1);
        assert_eq!(scores["g1"], vec![42.0]);
    }
}
