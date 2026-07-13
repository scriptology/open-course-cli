use tempfile::TempDir;

use open_course_cli::core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, FeedbackComment, GrammarError, GrammarErrorType,
    SentenceAnalysis, advance_exercise, apply_analysis, create_session, get_due_review_topics,
    get_weak_review_topics, is_session_complete, record_answer, select_side_topics,
    select_target_topics,
};
use open_course_cli::db::Database;
use open_course_cli::db::curriculum::{Curriculum, Difficulty, Topic};
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

fn make_exercise(target_topics: &[&str], side_topics: &[&str]) -> Exercise {
    Exercise {
        id: "e1".to_string(),
        target_sentence: "Hello".to_string(),
        expected_translation: "Привет".to_string(),
        target_topic_ids: target_topics.iter().map(|s| s.to_string()).collect(),
        side_topic_ids: side_topics.iter().map(|s| s.to_string()).collect(),
        expected_patterns: vec![],
        hint: None,
    }
}

#[test]
fn session_lifecycle() {
    let exercises = vec![
        make_exercise(&["t1"], &["t2"]),
        make_exercise(&["t1"], &["t3"]),
    ];
    let session = create_session(exercises, 2);
    assert_eq!(session.exercises.len(), 2);
    assert!(!is_session_complete(&session));

    let session = record_answer(&session, 0, "hello".to_string());
    assert_eq!(session.answers.get(&0).unwrap(), "hello");

    let session = advance_exercise(&session);
    assert_eq!(session.current_exercise_index, 1);
    assert!(!is_session_complete(&session));

    let session = advance_exercise(&session);
    assert!(is_session_complete(&session));
}

#[test]
fn topic_selection() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Intermediate),
        make_topic("t3", Difficulty::Advanced),
    ];
    let progress = ProgressData {
        version: 2,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 30.0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t2".to_string(),
                score: 70.0,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let due = get_due_review_topics(&topics, &progress, None);
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].id, "t3");

    let targets = select_target_topics(&topics, &progress, None);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, "t3");

    let sides = select_side_topics(&topics, &targets, 2);
    assert_eq!(sides.len(), 2);
    assert!(!sides.iter().any(|t| t.id == "t3"));
}

#[test]
fn weak_topics() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
        make_topic("t3", Difficulty::Beginner),
    ];
    let progress = ProgressData {
        version: 2,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 30.0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t3".to_string(),
                score: 60.0,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let weak = get_weak_review_topics(&topics, &progress);
    assert_eq!(weak.len(), 1);
    assert_eq!(weak[0].id, "t1");
}

#[tokio::test]
async fn apply_analysis_updates_progress() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let history = db.history();

    let curriculum = Curriculum {
        version: 1,
        topics: vec![
            make_topic("t1", Difficulty::Beginner),
            make_topic("t2", Difficulty::Beginner),
        ],
        target_language: "en".to_string(),
        native_language: "ru".to_string(),
    };

    let exercises = vec![
        make_exercise(&["t1"], &["t2"]),
        make_exercise(&["t1"], &["t2"]),
    ];
    let session = create_session(exercises, 2);

    let analysis = AnalysisResult {
        session_score: Some(85.0),
        sentences: vec![
            SentenceAnalysis {
                sentence_number: 1,
                student_translation: "Hi".to_string(),
                expected_translation: "Привет".to_string(),
                errors: vec![],
                per_sentence_feedback: vec![FeedbackComment {
                    comment: "Good".to_string(),
                }],
            },
            SentenceAnalysis {
                sentence_number: 2,
                student_translation: "Hello".to_string(),
                expected_translation: "Привет".to_string(),
                errors: vec![GrammarError {
                    error_type: GrammarErrorType::Minor,
                    pattern: "word order".to_string(),
                    explanation: "wrong order".to_string(),
                    ..Default::default()
                }],
                per_sentence_feedback: vec![FeedbackComment {
                    comment: "OK".to_string(),
                }],
            },
        ],
        evaluated_topics: vec![
            EvaluatedTopic {
                topic_id: "t1".to_string(),
                score: 90.0,
                previous_score: None,
            },
            EvaluatedTopic {
                topic_id: "t2".to_string(),
                score: 80.0,
                previous_score: None,
            },
        ],
        new_topics: vec![],
    };

    let mut progress = ProgressData {
        version: 2,
        topics: vec![],
        ..Default::default()
    };

    apply_analysis(&analysis, &session, &curriculum, &mut progress, &history)
        .await
        .unwrap();

    assert_eq!(progress.topics.len(), 2);
    assert_eq!(progress.session_count, 1);
    assert!(
        progress
            .adaptive_alerts
            .contains(&"review_session_errors".to_string())
    );
    let t1 = progress.topics.iter().find(|t| t.topic_id == "t1").unwrap();
    let t2 = progress.topics.iter().find(|t| t.topic_id == "t2").unwrap();

    assert!(t1.score > 0.0 && t1.score < 50.0);
    assert!(t2.score > 0.0 && t2.score < 50.0);
    assert_eq!(t1.score, t2.score);
    assert!(t1.last_practiced.is_some());

    let expected_t1 = {
        let s1: f64 = 0.0 * (1.0 - 0.12) + 100.0 * 0.12;
        let raw_s2: f64 = s1 * (1.0 - 0.12) + 47.0 * 0.12;
        // The erroneous exercise must not increase the score, so it stays at s1.
        assert!(raw_s2 > s1);
        s1.round()
    };
    assert_eq!(t1.score, expected_t1);

    let history_records = history.read_all().await.unwrap();
    assert_eq!(history_records.len(), 1);
    assert_eq!(history_records[0].avg_target_score, expected_t1);
}

#[tokio::test]
async fn errors_decrease_existing_score() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let history = db.history();

    let curriculum = Curriculum {
        version: 1,
        topics: vec![make_topic("t1", Difficulty::Beginner)],
        target_language: "en".to_string(),
        native_language: "ru".to_string(),
    };

    let exercises = vec![make_exercise(&["t1"], &[])];
    let session = create_session(exercises, 1);

    let analysis = AnalysisResult {
        session_score: Some(50.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: "Hi".to_string(),
            expected_translation: "Привет".to_string(),
            errors: vec![GrammarError {
                error_type: GrammarErrorType::Major,
                pattern: "grammar".to_string(),
                explanation: "wrong".to_string(),
                ..Default::default()
            }],
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![EvaluatedTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            previous_score: Some(50.0),
        }],
        new_topics: vec![],
    };

    let mut progress = ProgressData {
        version: 2,
        topics: vec![ProgressTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            last_practiced: None,
        }],
        ..Default::default()
    };

    apply_analysis(&analysis, &session, &curriculum, &mut progress, &history)
        .await
        .unwrap();

    let t1 = progress.topics.iter().find(|t| t.topic_id == "t1").unwrap();
    // Major error gives exercise_score = 50 - 8 = 42, so the score must drop from 50.
    assert!(t1.score < 50.0);
}

#[tokio::test]
async fn new_topic_with_errors_stays_at_zero() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let history = db.history();

    let curriculum = Curriculum {
        version: 1,
        topics: vec![make_topic("t1", Difficulty::Beginner)],
        target_language: "en".to_string(),
        native_language: "ru".to_string(),
    };

    let exercises = vec![make_exercise(&["t1"], &[])];
    let session = create_session(exercises, 1);

    let analysis = AnalysisResult {
        session_score: Some(50.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: "Hi".to_string(),
            expected_translation: "Привет".to_string(),
            errors: vec![GrammarError {
                error_type: GrammarErrorType::Minor,
                pattern: "grammar".to_string(),
                explanation: "wrong".to_string(),
                ..Default::default()
            }],
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![EvaluatedTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            previous_score: None,
        }],
        new_topics: vec![],
    };

    let mut progress = ProgressData {
        version: 2,
        topics: vec![],
        ..Default::default()
    };

    apply_analysis(&analysis, &session, &curriculum, &mut progress, &history)
        .await
        .unwrap();

    let t1 = progress.topics.iter().find(|t| t.topic_id == "t1").unwrap();
    assert_eq!(t1.score, 0.0);
}
