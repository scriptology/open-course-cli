//! Picking what to practice next: due reviews, weak topics and fresh
//! curriculum topics.

use std::collections::HashMap;

use chrono::Utc;

use crate::db::curriculum::Topic;
use crate::db::progress::{ProgressData, ProgressTopic};

use super::MASTERY_THRESHOLD;
use super::scoring::clamp_score;

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

/// Background review topics practiced alongside the session's target topic.
/// Only topics that still need practice qualify: effective mastery below
/// `MASTERY_THRESHOLD`, where topics without a progress record count as
/// fresh candidates with mastery 0 (same rule as `get_due_review_topics`).
/// Weakest first; ties break towards the least recently practiced topic
/// (never-practiced first). Returns fewer than `count` topics when not
/// enough qualify — the list is never padded, which keeps the side-topic
/// context rotating instead of repeating the first curriculum topics.
pub fn select_side_topics(
    topics: &[Topic],
    exclude: &[Topic],
    count: usize,
    progress: &ProgressData,
    now: chrono::DateTime<Utc>,
) -> Vec<Topic> {
    let exclude_ids: std::collections::HashSet<_> = exclude.iter().map(|t| &t.id).collect();
    let progress_map: HashMap<&String, &ProgressTopic> =
        progress.topics.iter().map(|t| (&t.topic_id, t)).collect();

    let mastery_of = |id: &String| -> f64 {
        progress_map
            .get(id)
            .map(|p| effective_mastery(p, now))
            .unwrap_or(0.0)
    };

    let mut candidates: Vec<&Topic> = topics
        .iter()
        .filter(|t| !exclude_ids.contains(&t.id) && mastery_of(&t.id) < MASTERY_THRESHOLD)
        .collect();

    candidates.sort_by(|a, b| {
        let score_cmp = mastery_of(&a.id)
            .partial_cmp(&mastery_of(&b.id))
            .unwrap_or(std::cmp::Ordering::Equal);
        if score_cmp != std::cmp::Ordering::Equal {
            return score_cmp;
        }
        let practiced_a = progress_map
            .get(&a.id)
            .and_then(|p| p.last_practiced.as_deref());
        let practiced_b = progress_map
            .get(&b.id)
            .and_then(|p| p.last_practiced.as_deref());
        match (practiced_a, practiced_b) {
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(da), Some(db)) => da.cmp(db),
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    candidates.into_iter().take(count).cloned().collect()
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
            Some(score) => *score < MASTERY_THRESHOLD,
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
            Some(pt) => effective_mastery(pt, now) < MASTERY_THRESHOLD,
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
/// topics whose effective mastery decayed below the mastery threshold,
/// weakest first. When both candidates exist, every third session is a
/// review (every second when the backlog is large), so new topics keep
/// flowing at a predictable pace.
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
            (mastery < MASTERY_THRESHOLD).then(|| (t.clone(), mastery))
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
