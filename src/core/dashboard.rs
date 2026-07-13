use std::collections::HashMap;

use chrono::NaiveDate;

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
pub struct DifficultyProgress {
    pub difficulty: String,
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub not_started: usize,
    pub percent: f64,
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
                if pt.score >= 80.0 {
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

pub fn get_progress_by_difficulty(
    curriculum: &Curriculum,
    progress: &ProgressData,
) -> Vec<DifficultyProgress> {
    let progress_map: HashMap<_, _> = progress.topics.iter().map(|t| (&t.topic_id, t)).collect();
    let difficulties = ["beginner", "intermediate", "advanced"];

    difficulties
        .iter()
        .map(|difficulty| {
            let topics: Vec<_> = curriculum
                .topics
                .iter()
                .filter(|t| t.difficulty == *difficulty)
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
                        if pt.score >= 80.0 {
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

            DifficultyProgress {
                difficulty: difficulty.to_string(),
                total,
                completed,
                in_progress,
                not_started,
                percent,
            }
        })
        .collect()
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
        if pt.score < 80.0 {
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
