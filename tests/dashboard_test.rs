use open_course_cli::core::dashboard::{
    get_course_progress, get_daily_activity, get_progress_by_difficulty,
};
use open_course_cli::db::curriculum::{Curriculum, Difficulty, Topic};
use open_course_cli::db::history::SessionSummary;
use open_course_cli::db::progress::{ProgressData, ProgressTopic};

fn make_topic(id: &str, difficulty: Difficulty) -> Topic {
    Topic {
        id: id.to_string(),
        name: id.to_string(),
        description: format!("{id} description"),
        difficulty: difficulty.as_str().to_string(),
        level: None,
        order: None,
        tags: vec![],
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
    }
}

#[test]
fn course_progress() {
    let curriculum = Curriculum {
        version: 1,
        topics: vec![
            make_topic("t1", Difficulty::Beginner),
            make_topic("t2", Difficulty::Beginner),
            make_topic("t3", Difficulty::Beginner),
        ],
        target_language: "en".to_string(),
        native_language: "ru".to_string(),
    };
    let progress = ProgressData {
        version: 2,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 90.0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t2".to_string(),
                score: 60.0,
                last_practiced: Some("2024-01-01".to_string()),
            },
        ],
        ..Default::default()
    };

    let result = get_course_progress(&curriculum, &progress);
    assert_eq!(result.completed, 1);
    assert_eq!(result.in_progress, 1);
    assert_eq!(result.not_started, 1);
    assert_eq!(result.total, 3);
    assert_eq!(result.percent, 50.0);
}

#[test]
fn progress_by_difficulty() {
    let curriculum = Curriculum {
        version: 1,
        topics: vec![
            make_topic("t1", Difficulty::Beginner),
            make_topic("t2", Difficulty::Beginner),
            make_topic("t3", Difficulty::Intermediate),
        ],
        target_language: "en".to_string(),
        native_language: "ru".to_string(),
    };
    let progress = ProgressData {
        version: 2,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 85.0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t3".to_string(),
                score: 75.0,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let by_difficulty = get_progress_by_difficulty(&curriculum, &progress);
    let beginner = by_difficulty
        .iter()
        .find(|d| d.difficulty == "beginner")
        .unwrap();
    assert_eq!(beginner.total, 2);
    assert_eq!(beginner.completed, 1);
    assert_eq!(beginner.in_progress, 0);
    assert_eq!(beginner.not_started, 1);

    let advanced = by_difficulty
        .iter()
        .find(|d| d.difficulty == "advanced")
        .unwrap();
    assert_eq!(advanced.total, 0);
}

#[test]
fn daily_activity() {
    use chrono::NaiveDate;

    let history = vec![
        SessionSummary {
            id: "1".to_string(),
            date: "2024-01-01T10:00:00Z".to_string(),
            target_topic_ids: vec![],
            side_topic_ids: vec![],
            new_topic_ids: vec!["t1".to_string(), "t2".to_string()],
            avg_target_score: 70.0,
            target_delta: 0.0,
        },
        SessionSummary {
            id: "2".to_string(),
            date: "2024-01-02T10:00:00Z".to_string(),
            target_topic_ids: vec![],
            side_topic_ids: vec![],
            new_topic_ids: vec![],
            avg_target_score: 85.0,
            target_delta: 0.0,
        },
    ];
    let progress = ProgressData {
        version: 2,
        topics: vec![ProgressTopic {
            topic_id: "t1".to_string(),
            score: 90.0,
            last_practiced: Some("2024-01-02T10:00:00Z".to_string()),
        }],
        ..Default::default()
    };

    let today = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
    let activity = get_daily_activity(&history, &progress, 3, today);
    assert_eq!(activity.len(), 3);

    let day1 = activity.iter().find(|a| a.date == "2024-01-01").unwrap();
    assert_eq!(day1.sessions, 1);
    assert_eq!(day1.new_topics, 2);
    assert_eq!(day1.completed_topics, 0);

    let day2 = activity.iter().find(|a| a.date == "2024-01-02").unwrap();
    assert_eq!(day2.sessions, 1);
    assert_eq!(day2.new_topics, 0);
    assert_eq!(day2.completed_topics, 1);

    let day3 = activity.iter().find(|a| a.date == "2024-01-03").unwrap();
    assert_eq!(day3.sessions, 0);
    assert_eq!(day3.new_topics, 0);
    assert_eq!(day3.completed_topics, 0);
}
