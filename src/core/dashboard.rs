use std::collections::HashMap;

use chrono::NaiveDate;

use crate::core::session::COMPLETED_THRESHOLD;
use crate::db::curriculum::Curriculum;
use crate::db::history::SessionSummary;
use crate::db::progress::ProgressData;

#[derive(Debug, Clone, PartialEq)]
pub struct CourseProgress {
    pub completed: usize,
    pub in_progress: usize,
    pub not_started: usize,
    pub total: usize,
    pub percent: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LevelProgress {
    pub level: String,
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub not_started: usize,
    pub percent: f64,
}

fn level_from_topic(topic: &crate::db::curriculum::Topic) -> String {
    topic
        .level
        .clone()
        .unwrap_or_else(|| match topic.difficulty.as_str() {
            "beginner" => "A1".to_string(),
            "intermediate" => "B1".to_string(),
            "advanced" => "C1".to_string(),
            _ => "A1".to_string(),
        })
}

/// Calculate a dynamic current CEFR level from per-level progress.
///
/// Each level is weighted by the amount of curriculum material at that level
/// multiplied by the average score (`total * percent`). The weighted average
/// of CEFR values (A1=1..C2=6) is rounded to the nearest level. This means a
/// level with many active topics pulls the result toward itself, while a
/// fully mastered but small level has less influence.
///
/// Returns `None` when there is no progress yet, so the UI can fall back to
/// the self-assessed CEFR from the profile.
pub fn calculate_current_level(levels: &[LevelProgress]) -> Option<String> {
    let cefr_order = ["A1", "A2", "B1", "B2", "C1", "C2"];
    let mut total_weight = 0.0;
    let mut weighted_sum = 0.0;

    for (idx, level) in levels.iter().enumerate() {
        if level.total == 0 || level.percent <= 0.0 {
            continue;
        }
        let weight = level.total as f64 * level.percent;
        let value = (idx + 1) as f64;
        total_weight += weight;
        weighted_sum += value * weight;
    }

    if total_weight == 0.0 {
        return None;
    }

    let avg = weighted_sum / total_weight;
    let level_idx = (avg.round() as usize).clamp(1, 6) - 1;
    Some(cefr_order[level_idx].to_string())
}

pub fn get_progress_by_level(
    curriculum: &Curriculum,
    progress: &ProgressData,
) -> Vec<LevelProgress> {
    let progress_map: HashMap<_, _> = progress.topics.iter().map(|t| (&t.topic_id, t)).collect();
    let levels = ["A1", "A2", "B1", "B2", "C1", "C2"];

    levels
        .iter()
        .map(|level| {
            let topics: Vec<_> = curriculum
                .topics
                .iter()
                .filter(|t| level_from_topic(t) == **level)
                .collect();
            let total = topics.len();
            let mut completed = 0;
            let mut in_progress = 0;
            let mut not_started = 0;
            let mut total_score = 0.0;

            for topic in topics {
                match progress_map.get(&topic.id) {
                    None => not_started += 1,
                    Some(pt) => {
                        total_score += pt.score;
                        if pt.score >= COMPLETED_THRESHOLD {
                            completed += 1;
                        } else if pt.last_practiced.is_some() {
                            in_progress += 1;
                        } else {
                            not_started += 1;
                        }
                    }
                }
            }

            let percent = if total > 0 {
                (total_score / total as f64).round()
            } else {
                0.0
            };

            LevelProgress {
                level: (*level).to_string(),
                total,
                completed,
                in_progress,
                not_started,
                percent,
            }
        })
        .collect()
}

pub fn get_course_progress(curriculum: &Curriculum, progress: &ProgressData) -> CourseProgress {
    let total = curriculum.topics.len();
    let progress_map: HashMap<_, _> = progress.topics.iter().map(|t| (&t.topic_id, t)).collect();

    let mut completed = 0;
    let mut in_progress = 0;
    let mut not_started = 0;
    let mut total_score = 0.0;

    for topic in &curriculum.topics {
        match progress_map.get(&topic.id) {
            None => not_started += 1,
            Some(pt) => {
                total_score += pt.score;
                if pt.score >= COMPLETED_THRESHOLD {
                    completed += 1;
                } else if pt.last_practiced.is_some() {
                    in_progress += 1;
                } else {
                    not_started += 1;
                }
            }
        }
    }

    let percent = if total > 0 {
        (total_score / total as f64).round()
    } else {
        0.0
    };

    CourseProgress {
        completed,
        in_progress,
        not_started,
        total,
        percent,
    }
}

pub fn get_session_trend(history: &[SessionSummary], limit: usize) -> Vec<f64> {
    if history.is_empty() {
        return Vec::new();
    }
    history
        .iter()
        .rev()
        .take(limit)
        .rev()
        .map(|s| s.avg_target_score.clamp(0.0, 100.0).round())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyActivity {
    pub date: String,
    pub sessions: usize,
    pub new_topics: usize,
    pub completed_topics: usize,
}

fn rfc3339_to_date(rfc: &str) -> Option<NaiveDate> {
    let date_part = rfc.split('T').next()?;
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()
}

pub fn get_daily_activity(
    history: &[SessionSummary],
    progress: &ProgressData,
    days: usize,
    today: NaiveDate,
) -> Vec<DailyActivity> {
    if days == 0 {
        return Vec::new();
    }

    let start = today - chrono::Duration::days(days as i64 - 1);

    let mut map: HashMap<NaiveDate, DailyActivity> = HashMap::new();
    for offset in 0..days {
        let date = start + chrono::Duration::days(offset as i64);
        map.insert(
            date,
            DailyActivity {
                date: date.to_string(),
                sessions: 0,
                new_topics: 0,
                completed_topics: 0,
            },
        );
    }

    for summary in history {
        if let Some(date) = rfc3339_to_date(&summary.date) {
            if let Some(activity) = map.get_mut(&date) {
                activity.sessions += 1;
                activity.new_topics += summary.new_topic_ids.len();
            }
        }
    }

    for pt in &progress.topics {
        if pt.score < COMPLETED_THRESHOLD {
            continue;
        }
        if let Some(rfc) = pt.last_practiced.as_deref() {
            if let Some(date) = rfc3339_to_date(rfc) {
                if let Some(activity) = map.get_mut(&date) {
                    activity.completed_topics += 1;
                }
            }
        }
    }

    let mut result: Vec<DailyActivity> = map.into_values().collect();
    result.sort_by(|a, b| a.date.cmp(&b.date));
    result
}
