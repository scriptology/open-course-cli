use open_course_cli::core::dashboard::{
    calculate_current_level, get_course_progress, get_daily_activity, get_progress_by_level,
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
        version: 3,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 90.0,
                mastery: 90.0,
                difficulty_estimate: 0.0,
                practice_count: 1,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t2".to_string(),
                score: 60.0,
                mastery: 60.0,
                difficulty_estimate: 0.0,
                practice_count: 1,
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
fn progress_by_level() {
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
        version: 3,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 85.0,
                mastery: 85.0,
                difficulty_estimate: 0.0,
                practice_count: 1,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t3".to_string(),
                score: 75.0,
                mastery: 75.0,
                difficulty_estimate: 0.0,
                practice_count: 1,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let by_level = get_progress_by_level(&curriculum, &progress);
    let a1 = by_level.iter().find(|d| d.level == "A1").unwrap();
    assert_eq!(a1.total, 2);
    assert_eq!(a1.completed, 1);
    assert_eq!(a1.in_progress, 0);
    assert_eq!(a1.not_started, 1);

    let c1 = by_level.iter().find(|d| d.level == "C1").unwrap();
    assert_eq!(c1.total, 0);
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
        version: 3,
        topics: vec![ProgressTopic {
            topic_id: "t1".to_string(),
            score: 90.0,
            mastery: 90.0,
            difficulty_estimate: 0.0,
            practice_count: 1,
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

#[test]
fn weak_selection_activates_and_wraps() {
    use open_course_cli::ui::views::DashboardState;

    let mut state = DashboardState::default();
    state.weak_topics = (1..=3)
        .map(|i| make_topic(&format!("t{i}"), Difficulty::Beginner))
        .collect();

    // Inactive: down selects the first row.
    state.move_weak_selection(1);
    assert_eq!(state.weak_selected, Some(0));

    state.move_weak_selection(1);
    assert_eq!(state.weak_selected, Some(1));

    // Wraps forward and backward.
    state.move_weak_selection(2);
    assert_eq!(state.weak_selected, Some(0));

    state.move_weak_selection(-1);
    assert_eq!(state.weak_selected, Some(2));

    // Fresh activation with up lands on the last row.
    state.weak_selected = None;
    state.move_weak_selection(-1);
    assert_eq!(state.weak_selected, Some(2));
}

#[test]
fn weak_selection_handles_empty_and_visible_window() {
    use open_course_cli::ui::views::DashboardState;

    let mut state = DashboardState::default();
    state.move_weak_selection(1);
    assert_eq!(state.weak_selected, None);

    // 8 topics but only 5 visible rows: selection wraps after the 5th.
    state.weak_topics = (1..=8)
        .map(|i| make_topic(&format!("t{i}"), Difficulty::Beginner))
        .collect();
    for _ in 0..5 {
        state.move_weak_selection(1);
    }
    assert_eq!(state.weak_selected, Some(4));
    state.move_weak_selection(1);
    assert_eq!(state.weak_selected, Some(0));
}

#[test]
fn weak_selection_defaults_to_first() {
    use open_course_cli::ui::views::DashboardState;

    let mut state = DashboardState::default();
    assert_eq!(state.weak_selected, None);

    state.weak_topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    assert_eq!(state.weak_visible_len(), 2);

    // After refresh() the first weak topic is selected by default.
    state.weak_selected = if state.weak_visible_len() > 0 {
        Some(0)
    } else {
        None
    };
    assert_eq!(state.weak_selected, Some(0));

    // Empty list keeps selection unset.
    state.weak_topics.clear();
    state.weak_selected = if state.weak_visible_len() > 0 {
        Some(0)
    } else {
        None
    };
    assert_eq!(state.weak_selected, None);
}

fn level_progress(level: &str, percent: f64) -> open_course_cli::core::dashboard::LevelProgress {
    open_course_cli::core::dashboard::LevelProgress {
        level: level.to_string(),
        total: 1,
        completed: 0,
        in_progress: 0,
        not_started: 0,
        percent,
    }
}

#[test]
fn current_level_returns_none_without_progress() {
    let levels = [
        level_progress("A1", 0.0),
        level_progress("A2", 0.0),
        level_progress("B1", 0.0),
        level_progress("B2", 0.0),
        level_progress("C1", 0.0),
        level_progress("C2", 0.0),
    ];
    assert_eq!(calculate_current_level(&levels), None);
}

#[test]
fn current_level_is_a1_when_only_a1_has_progress() {
    let levels = [
        level_progress("A1", 80.0),
        level_progress("A2", 0.0),
        level_progress("B1", 0.0),
        level_progress("B2", 0.0),
        level_progress("C1", 0.0),
        level_progress("C2", 0.0),
    ];
    assert_eq!(calculate_current_level(&levels), Some("A1".to_string()));
}

#[test]
fn current_level_rounds_weighted_average() {
    // A1=1 with 80%, A2=2 with 80% -> avg = (1*80 + 2*80) / 160 = 1.5 -> A2
    let levels = [
        level_progress("A1", 80.0),
        level_progress("A2", 80.0),
        level_progress("B1", 0.0),
        level_progress("B2", 0.0),
        level_progress("C1", 0.0),
        level_progress("C2", 0.0),
    ];
    assert_eq!(calculate_current_level(&levels), Some("A2".to_string()));
}

#[test]
fn current_level_can_go_down() {
    // A1 mastered, B1 barely touched -> weighted average closer to A1
    let levels = [
        level_progress("A1", 100.0),
        level_progress("A2", 0.0),
        level_progress("B1", 10.0),
        level_progress("B2", 0.0),
        level_progress("C1", 0.0),
        level_progress("C2", 0.0),
    ];
    assert_eq!(calculate_current_level(&levels), Some("A1".to_string()));
}

#[test]
fn current_level_trends_up_with_higher_level_progress() {
    // A1 not touched, A2 and B1 equally progressed -> avg = 2.5 -> B1
    let levels = [
        level_progress("A1", 0.0),
        level_progress("A2", 80.0),
        level_progress("B1", 80.0),
        level_progress("B2", 0.0),
        level_progress("C1", 0.0),
        level_progress("C2", 0.0),
    ];
    assert_eq!(calculate_current_level(&levels), Some("B1".to_string()));
}
