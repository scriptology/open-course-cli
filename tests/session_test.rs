use chrono::Utc;
use tempfile::TempDir;

use open_course_cli::core::session::{
    AnalysisResult, EvaluatedTopic, Exercise, FeedbackComment, GrammarError, GrammarErrorType,
    MentorSession, NextSessionTopic, SemanticVerdict, SentenceAnalysis, apply_analysis,
    apply_analysis_to_db, create_session, get_due_review_topics, get_weak_review_topics,
    pick_next_session_topic, select_side_topics, select_target_topics,
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
    let mut session = create_session(exercises, 2);
    assert_eq!(session.exercises.len(), 2);
    assert!(!session.is_complete());

    session.record_answer(0, "hello".to_string());
    assert_eq!(session.answers.get(&0).unwrap(), "hello");

    session.advance_exercise();
    assert_eq!(session.current_exercise_index, 1);
    assert!(!session.is_complete());

    session.advance_exercise();
    assert!(session.is_complete());
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

    let sides = select_side_topics(&topics, &targets, 2, &progress, Utc::now());
    // New behaviour: only weak/new topics qualify, no padding — t2 (70) is
    // excluded, so just t1 (30) is returned.
    assert_eq!(sides.len(), 1);
    assert_eq!(sides[0].id, "t1");
}

#[test]
fn side_topics_pick_weak_ones_without_padding() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
        make_topic("t3", Difficulty::Beginner),
        make_topic("t4", Difficulty::Beginner),
        make_topic("t5", Difficulty::Beginner),
    ];
    let progress = progress_with(
        vec![
            practiced("t1", 20.0, 0),
            practiced("t2", 45.0, 0),
            practiced("t3", 60.0, 0),
            practiced("t4", 80.0, 0),
            practiced("t5", 90.0, 0),
        ],
        0,
    );

    // Two weak topics out of five, weakest first; mastered topics are never
    // used to pad the list up to `count`.
    let sides = select_side_topics(&topics, &[], 3, &progress, Utc::now());
    let ids: Vec<&str> = sides.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2"]);
}

#[test]
fn side_topics_empty_when_all_mastered() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
    ];
    let progress = progress_with(vec![practiced("t1", 60.0, 0), practiced("t2", 75.0, 0)], 0);

    let sides = select_side_topics(&topics, &[], 3, &progress, Utc::now());
    assert!(sides.is_empty());
}

#[test]
fn side_topics_tie_break_by_recency() {
    let topics = vec![
        make_topic("t1", Difficulty::Beginner),
        make_topic("t2", Difficulty::Beginner),
        make_topic("t3", Difficulty::Beginner),
        make_topic("t4", Difficulty::Beginner),
    ];
    // t1, t2 and t3 tie at effective mastery 40; t4 has no progress record
    // at all and counts as a fresh candidate with mastery 0.
    let progress = progress_with(
        vec![
            practiced("t1", 40.0, 0),
            practiced("t2", 66.0, 10), // 66 * e^-0.5 ≈ 40 after decay
            ProgressTopic {
                topic_id: "t3".to_string(),
                score: 40.0,
                mastery: 40.0,
                difficulty_estimate: 0.0,
                practice_count: 0,
                last_practiced: None,
            },
        ],
        0,
    );

    let sides = select_side_topics(&topics, &[], 4, &progress, Utc::now());
    let ids: Vec<&str> = sides.iter().map(|t| t.id.as_str()).collect();
    // t4 first (no record → weakest), then the tie: t3 (never practiced)
    // before t2 (practiced longer ago) before t1.
    assert_eq!(ids, ["t4", "t3", "t2", "t1"]);
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

    apply_analysis(&analysis, &session, &mut progress, &history)
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

    apply_analysis(&analysis, &session, &mut progress, &history)
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

    apply_analysis(&analysis, &session, &mut progress, &history)
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

    apply_analysis_to_db(
        &analysis,
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

fn make_learning_item(id: &str, name: &str) -> LearningItem {
    LearningItem {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        level: None,
        target_lang: "es".to_string(),
        native_lang: "ru".to_string(),
        score: 0.0,
        last_practiced: None,
        practice_count: 0,
    }
}

/// A one-exercise session plus the matching analysis, for learning-item
/// scoring tests.
fn exercise_analysis(
    target_sentence: &str,
    expected: &str,
    student: &str,
    errors: Vec<GrammarError>,
) -> (MentorSession, AnalysisResult) {
    let exercises = vec![Exercise {
        id: "e1".to_string(),
        target_sentence: target_sentence.to_string(),
        expected_translation: expected.to_string(),
        acceptable_translations: vec![],
        target_topic_ids: vec![],
        side_topic_ids: vec![],
        expected_patterns: vec![],
        hint: None,
    }];
    let session = create_session(exercises, 1);
    let analysis = AnalysisResult {
        session_score: Some(80.0),
        sentences: vec![SentenceAnalysis {
            sentence_number: 1,
            student_translation: student.to_string(),
            expected_translation: expected.to_string(),
            acceptable_translations: vec![],
            semantic_verdict: if errors.is_empty() {
                SemanticVerdict::Correct
            } else {
                SemanticVerdict::NeedsCorrection
            },
            errors,
            per_sentence_feedback: vec![],
        }],
        evaluated_topics: vec![],
        new_topics: vec![],
        new_learning_items: vec![],
    };
    (session, analysis)
}

#[tokio::test]
async fn learning_item_not_in_session_keeps_stats_untouched() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let item = make_learning_item("es-pequeno-pequena", "pequeño/pequeña");
    db.learning_items().upsert(&item).await.unwrap();

    // The exercise never mentions the item's words.
    let (session, analysis) = exercise_analysis("Hello", "Привет", "Привет", vec![]);
    apply_analysis_to_db(
        &analysis,
        &session,
        &["es-pequeno-pequena".to_string()],
        &db,
    )
    .await
    .unwrap();

    let updated = db.learning_items().read_all().await.unwrap();
    assert_eq!(updated.len(), 1);
    // No occurrence -> no practice credit at all.
    assert_eq!(updated[0].score, 0.0);
    assert_eq!(updated[0].practice_count, 0);
    assert!(updated[0].last_practiced.is_none());
}

#[tokio::test]
async fn learning_item_in_session_without_errors_scores_100() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let item = make_learning_item("es-pequeno-pequena", "pequeño/pequeña");
    db.learning_items().upsert(&item).await.unwrap();

    let (session, analysis) =
        exercise_analysis("small table", "mesa pequeña", "mesa pequeña", vec![]);
    apply_analysis_to_db(
        &analysis,
        &session,
        &["es-pequeno-pequena".to_string()],
        &db,
    )
    .await
    .unwrap();

    let updated = db.learning_items().read_all().await.unwrap();
    assert_eq!(updated.len(), 1);
    // EMA from 0 towards 100 with alpha 0.12 -> 12.
    assert_eq!(updated[0].score, 12.0);
    assert_eq!(updated[0].practice_count, 1);
    assert!(updated[0].last_practiced.is_some());
}

#[tokio::test]
async fn learning_item_mentioned_in_error_scores_0() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let item = make_learning_item("es-caro-rico", "Caro vs Rico");
    db.learning_items().upsert(&item).await.unwrap();

    let errors = vec![GrammarError {
        error_type: GrammarErrorType::Major,
        pattern: "caro vs rico".to_string(),
        explanation: "wrong word".to_string(),
        ..Default::default()
    }];
    // The item's words occur in the student translation ("rico") and in the
    // error pattern.
    let (session, analysis) = exercise_analysis(
        "The chairs were expensive",
        "Las sillas eran caras",
        "Las sillas eran rico",
        errors,
    );
    apply_analysis_to_db(&analysis, &session, &["es-caro-rico".to_string()], &db)
        .await
        .unwrap();

    let updated = db.learning_items().read_all().await.unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].score, 0.0);
    assert_eq!(updated[0].practice_count, 1);
}

#[tokio::test]
async fn learning_item_matches_by_significant_words_not_full_name() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let item = make_learning_item("es-vsluh-aloud", "Adverb: вслух → out loud/aloud");
    db.learning_items().upsert(&item).await.unwrap();

    let errors = vec![GrammarError {
        error_type: GrammarErrorType::Minor,
        pattern: "вслух перевод".to_string(),
        explanation: "should be aloud".to_string(),
        ..Default::default()
    }];
    // The full item name never appears anywhere; the words "вслух"/"out loud"
    // occur in the exercise and the error mentions "вслух"/"aloud".
    let (session, analysis) = exercise_analysis(
        "She said it out loud",
        "Она сказала это вслух",
        "Она сказала это громко",
        errors,
    );
    apply_analysis_to_db(&analysis, &session, &["es-vsluh-aloud".to_string()], &db)
        .await
        .unwrap();

    let updated = db.learning_items().read_all().await.unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].score, 0.0);
    assert_eq!(updated[0].practice_count, 1);
}

#[tokio::test]
async fn new_learning_item_deduped_into_existing() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let mut existing = make_learning_item("es-remind-notice", "Verbs: remind vs notice");
    existing.score = 30.0;
    existing.practice_count = 2;
    db.learning_items().upsert(&existing).await.unwrap();

    let new_item = make_learning_item("es-remind-notice-2", "Verb: remind vs notice");
    let (session, mut analysis) = exercise_analysis(
        "Remind me to notice",
        "Recuérdame notarlo",
        "Recuérdame notarlo",
        vec![],
    );
    analysis.new_learning_items = vec![new_item];

    apply_analysis_to_db(&analysis, &session, &[], &db)
        .await
        .unwrap();

    let items = db.learning_items().read_all().await.unwrap();
    // No duplicate created; the existing item got the practice credit.
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "es-remind-notice");
    assert_eq!(items[0].practice_count, 3);
    assert!(items[0].score > 30.0);
}

#[tokio::test]
async fn new_topic_deduped_into_existing_curriculum_topic() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();

    let mut existing_topic = make_topic("conj-patterns", Difficulty::Beginner);
    existing_topic.name = "Conjugation patterns".to_string();
    db.curriculum().upsert(&existing_topic).await.unwrap();

    let mut dup_topic = make_topic("conj-patterns-2", Difficulty::Beginner);
    dup_topic.name = "Conjugation Patterns".to_string();

    let (session, mut analysis) = exercise_analysis("Hello", "Привет", "Привет", vec![]);
    analysis.new_topics = vec![dup_topic];

    apply_analysis_to_db(&analysis, &session, &[], &db)
        .await
        .unwrap();

    let curriculum = db.curriculum().read_all().await.unwrap();
    assert_eq!(curriculum.topics.len(), 1);
    assert_eq!(curriculum.topics[0].id, "conj-patterns");

    // The existing topic gets a progress entry, the duplicate does not.
    let progress = db.progress().read_all().await.unwrap();
    assert!(
        progress
            .topics
            .iter()
            .any(|p| p.topic_id == "conj-patterns")
    );
    assert!(
        !progress
            .topics
            .iter()
            .any(|p| p.topic_id == "conj-patterns-2")
    );
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

    apply_analysis(&analysis, &session, &mut progress, &history)
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
    let progress = progress_with(
        (1..=5)
            .map(|i| practiced(&format!("d{i}"), 20.0, 0))
            .collect(),
        1,
    );

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
