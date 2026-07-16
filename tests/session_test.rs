use chrono::Utc;
use tempfile::TempDir;

use open_course_cli::core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, FeedbackComment, GrammarError, GrammarErrorType,
    NextSessionTopic, SemanticVerdict, SentenceAnalysis, advance_exercise, apply_analysis,
    apply_analysis_to_db, create_session, get_due_review_topics, get_weak_review_topics,
    is_session_complete, pick_next_session_topic, record_answer, select_side_topics,
    select_target_topics,
};
use open_course_cli::db::Database;
use open_course_cli::db::curriculum::{Curriculum, Difficulty, Topic};
use open_course_cli::db::learning_items::LearningItem;
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
        acceptable_translations: vec![],
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
                mastery: 30.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t2".to_string(),
                score: 70.0,
                mastery: 70.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let due = get_due_review_topics(&topics, &progress, None, Utc::now());
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
        version: 3,
        topics: vec![
            ProgressTopic {
                topic_id: "t1".to_string(),
                score: 30.0,
                mastery: 30.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            },
            ProgressTopic {
                topic_id: "t3".to_string(),
                score: 60.0,
                mastery: 60.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            },
        ],
        ..Default::default()
    };

    let weak = get_weak_review_topics(&topics, &progress, Utc::now());
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
                acceptable_translations: vec![],
                semantic_verdict: open_course_cli::core::session::SemanticVerdict::Correct,
                errors: vec![],
                per_sentence_feedback: vec![FeedbackComment {
                    comment: "Good".to_string(),
                }],
            },
            SentenceAnalysis {
                sentence_number: 2,
                student_translation: "Hello".to_string(),
                expected_translation: "Привет".to_string(),
                acceptable_translations: vec![],
                semantic_verdict: open_course_cli::core::session::SemanticVerdict::NeedsCorrection,
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
        new_learning_items: vec![],
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

    assert!(t1.score > 0.0 && t1.score < 60.0);
    assert!(t2.score > 0.0 && t2.score < 60.0);
    assert_eq!(t1.score, t2.score);
    assert!(t1.last_practiced.is_some());

    let expected_t1 = {
        let alpha1 = 0.1 + 0.35 * (1.0 - 0.0 / 100.0);
        let s1: f64 = 0.0 * (1.0 - alpha1) + 100.0 * alpha1;
        let alpha2 = 0.1 + 0.35 * (1.0 - s1 / 100.0);
        let s2 = s1 * (1.0 - alpha2) + 47.0 * alpha2;
        s2.round()
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
            acceptable_translations: vec![],
            semantic_verdict: SemanticVerdict::NeedsCorrection,
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
        new_learning_items: vec![],
    };

    let mut progress = ProgressData {
        version: 3,
        topics: vec![ProgressTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            mastery: 50.0,
            difficulty_estimate: 0.0,
            practice_count: 0,
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
async fn new_topic_with_errors_grows_mastery() {
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
            acceptable_translations: vec![],
            semantic_verdict: SemanticVerdict::NeedsCorrection,
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
        new_learning_items: vec![],
    };

    let mut progress = ProgressData {
        version: 3,
        topics: vec![],
        ..Default::default()
    };

    apply_analysis(&analysis, &session, &curriculum, &mut progress, &history)
        .await
        .unwrap();

    let t1 = progress.topics.iter().find(|t| t.topic_id == "t1").unwrap();
    // Minor error gives exercise_score = 47; without the ratchet mastery grows from 0.
    assert!(t1.score > 0.0 && t1.score < 50.0);
}

#[tokio::test]
async fn apply_analysis_to_db_updates_learning_items() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let item = LearningItem {
        id: "es-pequeno-pequena".to_string(),
        name: "pequeño/pequeña".to_string(),
        description: "Adjective agreement".to_string(),
        level: Some("A1".to_string()),
        target_lang: "es".to_string(),
        native_lang: "ru".to_string(),
        score: 0.0,
        last_practiced: None,
        practice_count: 0,
    };
    db.learning_items().upsert(&item).await.unwrap();

    let curriculum = Curriculum {
        version: 1,
        topics: vec![make_topic("t1", Difficulty::Beginner)],
        target_language: "es".to_string(),
        native_language: "ru".to_string(),
    };
    for topic in &curriculum.topics {
        db.curriculum().upsert(topic).await.unwrap();
    }

    let exercises = vec![Exercise {
        id: "e1".to_string(),
        target_sentence: "small table".to_string(),
        expected_translation: "mesa pequeña".to_string(),
        acceptable_translations: vec![],
        target_topic_ids: vec!["t1".to_string()],
        side_topic_ids: vec![],
        expected_patterns: vec![],
        hint: None,
    }];
    let session = create_session(exercises, 1);

    let analysis = AnalysisResult {
        session_score: Some(50.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: "mesa pequeño".to_string(),
            expected_translation: "mesa pequeña".to_string(),
            acceptable_translations: vec![],
            semantic_verdict: SemanticVerdict::NeedsCorrection,
            errors: vec![GrammarError {
                error_type: GrammarErrorType::Minor,
                pattern: "pequeño/pequeña".to_string(),
                explanation: "agreement".to_string(),
                ..Default::default()
            }],
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![],
        new_topics: vec![],
        new_learning_items: vec![],
    };

    apply_analysis_to_db(&analysis,
        &session,
        &["es-pequeno-pequena".to_string()],
        &db,
    )
    .await
    .unwrap();

    let updated = db.learning_items().read_all().await.unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].practice_count, 1);
    assert!(updated[0].score < 50.0);
    assert!(updated[0].last_practiced.is_some());
}

#[tokio::test]
async fn apply_analysis_to_db_routes_word_topics_to_learning_items() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let curriculum = Curriculum {
        version: 1,
        topics: vec![make_topic("t1", Difficulty::Beginner)],
        target_language: "es".to_string(),
        native_language: "ru".to_string(),
    };
    for topic in &curriculum.topics {
        db.curriculum().upsert(topic).await.unwrap();
    }

    let exercises = vec![Exercise {
        id: "e1".to_string(),
        target_sentence: "The red chairs were very expensive".to_string(),
        expected_translation: "Las sillas rojas eran muy caras".to_string(),
        acceptable_translations: vec![],
        target_topic_ids: vec!["t1".to_string()],
        side_topic_ids: vec![],
        expected_patterns: vec![],
        hint: None,
    }];
    let session = create_session(exercises, 1);

    let word_topic = Topic {
        id: "adjective-caro-vs-rico".to_string(),
        name: "Adjective: Caro vs Rico".to_string(),
        description: "Confusion between caro (expensive) and rico (rich/tasty)".to_string(),
        difficulty: "beginner".to_string(),
        level: Some("A1".to_string()),
        order: None,
        tags: vec![],
        target_lang: "es".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
    };

    let analysis = AnalysisResult {
        session_score: Some(50.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: "Los sillas rojas eran muy ricos".to_string(),
            expected_translation: "Las sillas rojas eran muy caras".to_string(),
            acceptable_translations: vec![],
            semantic_verdict: SemanticVerdict::NeedsCorrection,
            errors: vec![GrammarError {
                error_type: GrammarErrorType::Major,
                pattern: "caro vs rico".to_string(),
                explanation: "wrong word".to_string(),
                ..Default::default()
            }],
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![],
        new_topics: vec![word_topic],
        new_learning_items: vec![],
    };

    apply_analysis_to_db(&analysis, &session, &[], &db)
        .await
        .unwrap();

    // A word-specific topic must not land in the curriculum or progress...
    let curriculum_after = db.curriculum().read_all().await.unwrap();
    assert!(
        !curriculum_after
            .topics
            .iter()
            .any(|t| t.name == "Adjective: Caro vs Rico")
    );
    let progress = db.progress().read_all().await.unwrap();
    assert!(
        !progress
            .topics
            .iter()
            .any(|t| t.topic_id == "adjective-caro-vs-rico")
    );

    // ...but must be stored as a learning item for review.
    let items = db.learning_items().read_all().await.unwrap();
    let item = items
        .iter()
        .find(|i| i.name == "Adjective: Caro vs Rico")
        .unwrap();
    let item_id = item.id.clone();

    // Re-running the same analysis must not reset the item's progress.
    let mut practiced = item.clone();
    practiced.score = 42.0;
    practiced.practice_count = 3;
    db.learning_items().upsert(&practiced).await.unwrap();

    apply_analysis_to_db(&analysis, &session, &[], &db)
        .await
        .unwrap();

    let items = db.learning_items().read_all().await.unwrap();
    let item = items.iter().find(|i| i.id == item_id).unwrap();
    assert_eq!(item.score, 42.0);
    assert_eq!(item.practice_count, 3);
}

#[test]
fn effective_mastery_decays_over_time() {
    let topic = ProgressTopic {
        topic_id: "t1".to_string(),
        score: 80.0,
        mastery: 80.0,
        difficulty_estimate: 0.0,
        practice_count: 1,
        last_practiced: Some("2024-01-01T00:00:00Z".to_string()),
    };
    let just_after = chrono::DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let later = chrono::DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let effective_now = open_course_cli::core::session::effective_mastery(&topic, just_after);
    let effective_later = open_course_cli::core::session::effective_mastery(&topic, later);

    assert_eq!(effective_now, 80.0);
    assert!(effective_later < 80.0);
    assert!(effective_later > 0.0);
}

#[tokio::test]
async fn acceptable_translation_grows_mastery() {
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
        session_score: Some(90.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: "Hi there".to_string(),
            expected_translation: "Привет".to_string(),
            acceptable_translations: vec!["Здравствуй".to_string()],
            semantic_verdict: SemanticVerdict::Acceptable,
            errors: vec![GrammarError {
                error_type: GrammarErrorType::Minor,
                pattern: "register".to_string(),
                explanation: "slightly informal".to_string(),
                ..Default::default()
            }],
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![],
        new_topics: vec![],
        new_learning_items: vec![],
    };

    let mut progress = ProgressData {
        version: 3,
        topics: vec![ProgressTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            mastery: 50.0,
            difficulty_estimate: 0.0,
            practice_count: 0,
            last_practiced: None,
        }],
        ..Default::default()
    };

    apply_analysis(&analysis, &session, &curriculum, &mut progress, &history)
        .await
        .unwrap();

    let t1 = progress.topics.iter().find(|t| t.topic_id == "t1").unwrap();
    // Acceptable with a minor note should still increase mastery from 50.
    assert!(t1.score > 50.0);
}


fn practiced(id: &str, mastery: f64, days_ago: i64) -> ProgressTopic {
    ProgressTopic {
        topic_id: id.to_string(),
        score: mastery,
        mastery,
        difficulty_estimate: 0.0,
        practice_count: 1,
        last_practiced: Some((Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339()),
    }
}

fn progress_with(topics: Vec<ProgressTopic>, session_count: i32) -> ProgressData {
    ProgressData {
        version: 3,
        topics,
        session_count,
        ..Default::default()
    }
}

#[test]
fn pick_next_prefers_new_topic_when_nothing_due() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    let progress = progress_with(vec![practiced("t1", 80.0, 1)], 0);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::New(t) => assert_eq!(t.id, "t2"),
        other => panic!("expected New(t2), got {other:?}"),
    }
}

#[test]
fn pick_next_reviews_every_third_session() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    // Upcoming session #3 with a due topic -> review, even though a new topic exists.
    let progress = progress_with(vec![practiced("t1", 30.0, 0)], 2);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::Review(t) => assert_eq!(t.id, "t1"),
        other => panic!("expected Review(t1), got {other:?}"),
    }
}

#[test]
fn pick_next_prefers_new_on_non_review_sessions() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    for session_count in [0, 1] {
        let progress = progress_with(vec![practiced("t1", 30.0, 0)], session_count);
        match pick_next_session_topic(&topics, &progress, Utc::now()) {
            NextSessionTopic::New(t) => assert_eq!(t.id, "t2"),
            other => panic!("expected New(t2) at session_count {session_count}, got {other:?}"),
        }
    }
}

#[test]
fn pick_next_reviews_every_second_session_on_backlog() {
    let mut topics: Vec<_> = (1..=5)
        .map(|i| make_topic(&format!("d{i}"), Difficulty::Beginner))
        .collect();
    topics.push(make_topic("n1", Difficulty::Beginner));
    // Upcoming session #2 with 5 due topics -> review.
    let progress = progress_with((1..=5).map(|i| practiced(&format!("d{i}"), 20.0, 0)).collect(), 1);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::Review(t) => assert_eq!(t.id, "d1"),
        other => panic!("expected Review(d1), got {other:?}"),
    }
}

#[test]
fn pick_next_weakest_due_first_with_decay() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    // t2 was stronger but practiced 10 days ago: 60 * e^-0.5 ≈ 36 < 45 = t1.
    let progress = progress_with(vec![practiced("t1", 45.0, 0), practiced("t2", 60.0, 10)], 2);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::Review(t) => assert_eq!(t.id, "t2"),
        other => panic!("expected Review(t2), got {other:?}"),
    }
}

#[test]
fn pick_next_reviews_when_no_new_topics() {
    let topics = vec![make_topic("t1", Difficulty::Beginner)];
    let progress = progress_with(vec![practiced("t1", 30.0, 0)], 0);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::Review(t) => assert_eq!(t.id, "t1"),
        other => panic!("expected Review(t1), got {other:?}"),
    }
}

#[test]
fn pick_next_extends_curriculum_when_nothing_to_do() {
    let topics = vec![make_topic("t1", Difficulty::Beginner)];
    let progress = progress_with(vec![practiced("t1", 90.0, 0)], 0);

    match pick_next_session_topic(&topics, &progress, Utc::now()) {
        NextSessionTopic::ExtendCurriculum => {}
        other => panic!("expected ExtendCurriculum, got {other:?}"),
    }
}
