mod apply;
mod models;
mod scoring;
mod topic_selection;

use std::collections::{HashMap, HashSet};

use chrono::Utc;

pub use apply::{apply_analysis, apply_analysis_to_db};
pub use models::{
    AnalysisResult, EvaluatedTopic, Exercise, FeedbackComment, GrammarError, GrammarErrorType,
    NewTopicRef, SemanticVerdict, SentenceAnalysis,
};
pub use scoring::{
    average, clamp_score, collect_topic_errors, count_topic_occurrences, error_based_score,
    error_penalty, recent_success_rate, weighted_ema_score,
};
pub use topic_selection::{
    NextSessionTopic, effective_mastery, get_due_review_topics, get_weak_review_topics,
    pick_next_session_topic, select_side_topics, select_target_topics,
};

/// Score below which a topic counts as weak and due for spaced review.
pub const MASTERY_THRESHOLD: f64 = 50.0;
/// Score at or above which a topic counts as completed on the dashboard.
pub const COMPLETED_THRESHOLD: f64 = 80.0;
/// Average target score below which a session raises a low-score alert.
pub const LOW_SESSION_SCORE_THRESHOLD: f64 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct MentorSession {
    pub id: String,
    pub exercises: Vec<Exercise>,
    pub answers: HashMap<usize, String>,
    pub current_exercise_index: usize,
}

impl MentorSession {
    /// Records the student's answer for the exercise at `index`.
    pub fn record_answer(&mut self, index: usize, answer: String) {
        self.answers.insert(index, answer);
    }

    /// Moves on to the next exercise.
    pub fn advance_exercise(&mut self) {
        self.current_exercise_index += 1;
    }

    /// Whether the session has run through all of its exercises.
    pub fn is_complete(&self) -> bool {
        self.current_exercise_index >= self.exercises.len()
    }
}

pub fn create_session(exercises: Vec<Exercise>, batch_size: usize) -> MentorSession {
    MentorSession {
        id: format!("{}", Utc::now().timestamp_millis()),
        exercises: exercises.into_iter().take(batch_size).collect(),
        answers: HashMap::new(),
        current_exercise_index: 0,
    }
}

/// Unique ids in order of first occurrence. The order is deterministic
/// because it feeds into `SessionSummary.target_topic_ids` / `side_topic_ids`.
pub fn unique_topic_ids<I>(ids: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for id in ids {
        if seen.insert(id.clone()) {
            unique.push(id);
        }
    }
    unique
}
