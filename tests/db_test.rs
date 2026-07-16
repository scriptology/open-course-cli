use tempfile::TempDir;

use open_course_cli::db::{
    Database,
    curriculum::{Difficulty, Topic},
    history::SessionSummary,
    learning_items::{LearningItem, LearningItemsTable},
    progress::ProgressTopic,
    reviews::TopicReview,
};

#[tokio::test]
async fn curriculum_crud() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let table = db.curriculum();

    let topic = Topic {
        id: "t1".to_string(),
        name: "Greetings".to_string(),
        description: "Basic greetings".to_string(),
        difficulty: Difficulty::Beginner.as_str().to_string(),
        level: None,
        order: None,
        tags: vec!["vocabulary".to_string()],
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
    };

    table.upsert(&topic).await.unwrap();
    let curriculum = table.read_all().await.unwrap();
    assert_eq!(curriculum.topics.len(), 1);
    assert_eq!(curriculum.topics[0].id, "t1");
    assert_eq!(curriculum.topics[0].tags, vec!["vocabulary".to_string()]);

    let updated = Topic {
        name: "Greetings & Farewells".to_string(),
        ..topic
    };
    table.upsert(&updated).await.unwrap();
    let curriculum = table.read_all().await.unwrap();
    assert_eq!(curriculum.topics.len(), 1);
    assert_eq!(curriculum.topics[0].name, "Greetings & Farewells");

    table.reset().await.unwrap();
    let curriculum = table.read_all().await.unwrap();
    assert!(curriculum.topics.is_empty());
}

#[tokio::test]
async fn progress_crud() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let table = db.progress();

    let topic = ProgressTopic {
        topic_id: "t1".to_string(),
        score: 75.0,
        mastery: 75.0,
        difficulty_estimate: 0.0,
        practice_count: 2,
        last_practiced: Some("2024-01-01T00:00:00Z".to_string()),
    };

    table.upsert(&topic).await.unwrap();
    let data = table.read_all().await.unwrap();
    assert_eq!(data.topics.len(), 1);
    assert_eq!(data.topics[0].topic_id, "t1");
    assert_eq!(data.topics[0].score, 75.0);

    let mut data = table.read_all().await.unwrap();
    data.session_count = 3;
    data.adaptive_alerts = vec!["keep practicing".to_string()];
    table.write_all(&data).await.unwrap();
    let data = table.read_all().await.unwrap();
    assert_eq!(data.session_count, 3);
    assert_eq!(data.adaptive_alerts, vec!["keep practicing".to_string()]);

    let fetched = table.get_by_topic_id("t1").await.unwrap().unwrap();
    assert_eq!(fetched.score, 75.0);

    let updated = ProgressTopic {
        score: 85.0,
        ..topic
    };
    table.upsert(&updated).await.unwrap();
    let data = table.read_all().await.unwrap();
    assert_eq!(data.topics.len(), 1);
    assert_eq!(data.topics[0].score, 85.0);

    table.reset().await.unwrap();
    let data = table.read_all().await.unwrap();
    assert!(data.topics.is_empty());
}

#[tokio::test]
async fn history_append_and_read() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let table = db.history();

    let s1 = SessionSummary {
        id: "1".to_string(),
        date: "2024-01-01".to_string(),
        target_topic_ids: vec!["t1".to_string()],
        side_topic_ids: vec!["t2".to_string()],
        new_topic_ids: vec!["t4".to_string()],
        avg_target_score: 80.0,
        target_delta: 5.0,
    };
    let s2 = SessionSummary {
        id: "2".to_string(),
        date: "2024-01-02".to_string(),
        target_topic_ids: vec!["t1".to_string()],
        side_topic_ids: vec!["t3".to_string()],
        new_topic_ids: vec![],
        avg_target_score: 85.0,
        target_delta: 5.0,
    };

    table.append(&s1).await.unwrap();
    table.append(&s2).await.unwrap();

    let all = table.read_all().await.unwrap();
    assert_eq!(all.len(), 2);

    let last = table.read_last(1).await.unwrap();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].id, "2");

    table.reset().await.unwrap();
    let all = table.read_all().await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn reviews_crud() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let table = db.reviews();

    let review = TopicReview {
        topic_id: "t1".to_string(),
        content: "Greetings are used to say hello.".to_string(),
        generated_at: "2024-01-01".to_string(),
    };

    table.upsert(&review).await.unwrap();
    let fetched = table.get_by_topic_id("t1").await.unwrap().unwrap();
    assert_eq!(fetched.content, review.content);

    let updated = TopicReview {
        content: "Updated content.".to_string(),
        ..review
    };
    table.upsert(&updated).await.unwrap();
    let fetched = table.get_by_topic_id("t1").await.unwrap().unwrap();
    assert_eq!(fetched.content, "Updated content.");

    table.remove_by_topic_id("t1").await.unwrap();
    let fetched = table.get_by_topic_id("t1").await.unwrap();
    assert!(fetched.is_none());
}

#[tokio::test]
async fn learning_items_crud() {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let table = db.learning_items();

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

    table.upsert(&item).await.unwrap();
    let all = table.read_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "es-pequeno-pequena");

    let updated = LearningItem {
        score: 45.0,
        last_practiced: Some("2024-01-01T00:00:00Z".to_string()),
        practice_count: 1,
        ..item.clone()
    };
    table.upsert(&updated).await.unwrap();
    let all = table.read_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].score, 45.0);

    let weak = LearningItemsTable::weakest(&all, 1);
    assert_eq!(weak.len(), 1);
    assert_eq!(weak[0].id, "es-pequeno-pequena");

    table.reset().await.unwrap();
    let all = table.read_all().await.unwrap();
    assert!(all.is_empty());
}
