//! Live end-to-end test against a real sync server. Skipped unless the env
//! vars are set:
//!
//! ```sh
//! OPEN_COURSE_SYNC_URL=http://localhost:8080 OC_LIVE_TOKEN=<bearer> \
//! OC_LIVE_PAIR_WITH_TOPICS=ru-fr OC_LIVE_PAIR_EMPTY=ru-es \
//!     cargo test -p open-course-sync --test live
//! ```
//!
//! `OC_LIVE_PAIR_WITH_TOPICS` must exist on the server with at least one
//! topic; `OC_LIVE_PAIR_EMPTY` must exist with an empty canonical curriculum
//! (e.g. created via `POST /v1/pairs`, like the web app does).

use open_course_core::curriculum::Topic;
use open_course_db::Database;
use open_course_sync::{BindScenario, PushError, SyncClient, backfill_outbox};

fn client() -> Option<SyncClient> {
    let base = std::env::var("OPEN_COURSE_SYNC_URL").ok()?;
    let token = std::env::var("OC_LIVE_TOKEN").ok()?;
    Some(SyncClient::new(base).ok()?.with_access_token(token))
}

fn topic(id: &str, native: &str, target: &str) -> Topic {
    Topic {
        id: id.to_string(),
        name: format!("Topic {id}"),
        description: "live test".to_string(),
        difficulty: "beginner".to_string(),
        level: None,
        order: None,
        tags: vec![],
        target_lang: target.to_string(),
        native_lang: native.to_string(),
        version: 1,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        deleted_at: None,
    }
}

async fn temp_db(dir: &tempfile::TempDir) -> Database {
    Database::connect(&dir.path().join("db")).await.unwrap()
}

/// `GET /v1/pairs` parses and returns the account's pairs — the discovery
/// feed for pairs created on the web.
#[tokio::test]
async fn list_pairs_parses() {
    let Some(client) = client() else {
        eprintln!("OPEN_COURSE_SYNC_URL/OC_LIVE_TOKEN unset, skipping");
        return;
    };
    let pairs = client.list_pairs().await.unwrap();
    assert!(!pairs.is_empty(), "expected at least one pair");
    let pair = &pairs[0];
    assert!(pair.pair_id.contains('-'));
    assert!(!pair.native_lang.is_empty());
    assert!(!pair.target_lang.is_empty());
}

/// Binding an empty local database to a pair that has cloud topics:
/// FreshCloud, then pull applies the topic.
#[tokio::test]
async fn bind_pulls_web_created_pair() {
    let Some(client) = client() else {
        eprintln!("OPEN_COURSE_SYNC_URL/OC_LIVE_TOKEN unset, skipping");
        return;
    };
    let pair_id = std::env::var("OC_LIVE_PAIR_WITH_TOPICS").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = temp_db(&dir).await;

    let scenario = client.first_bind_choices(&db, &pair_id).await.unwrap();
    assert!(
        matches!(scenario, BindScenario::FreshCloud),
        "expected FreshCloud, got {scenario:?}"
    );
    let revision = client.pull(&db, &pair_id).await.unwrap();
    assert!(revision > 0);
    let curriculum = db.curriculum().read_all().await.unwrap();
    assert!(
        !curriculum.topics.is_empty(),
        "pull must apply the cloud topics"
    );
}

/// A pair created on the web has an existing but possibly EMPTY canonical
/// curriculum (`canonical_topics = '[]'`), so the first push of local topics
/// is a 409 by design (`base_revision = 0` + existing canon). The client
/// must resolve it with `merge_bind`, not fail.
#[tokio::test]
async fn first_push_against_empty_canon_conflicts_then_merges() {
    let Some(client) = client() else {
        eprintln!("OPEN_COURSE_SYNC_URL/OC_LIVE_TOKEN unset, skipping");
        return;
    };
    let pair_id = std::env::var("OC_LIVE_PAIR_EMPTY").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = temp_db(&dir).await;

    // Unique per run: the pair's canon accumulates the merged topics.
    let topic_id = format!("live-local-{}", chrono::Utc::now().timestamp_millis());
    db.curriculum()
        .upsert_with_timestamps(&topic(&topic_id, "ru", "es"))
        .await
        .unwrap();
    backfill_outbox(&db, true).await.unwrap();

    match client.push(&db, &pair_id).await {
        Err(PushError::CurriculumConflict(canonical)) => {
            assert!(
                !canonical.topics.iter().any(|t| t.id == topic_id),
                "the canon must not contain the unpushed topic"
            );
        }
        other => panic!("expected a curriculum conflict, got {other:?}"),
    }

    let report = client.merge_bind(&db, &pair_id).await.unwrap();
    assert!(report.topics_local_only >= 1);
    let curriculum = db.curriculum().read_all().await.unwrap();
    assert!(curriculum.topics.iter().any(|t| t.id == topic_id));
}

/// Pulling a pair the account does not have yields revision 0 and no
/// changes instead of an error — the client must not mistake that for
/// "synced".
#[tokio::test]
async fn unknown_pair_pull_is_empty() {
    let Some(client) = client() else {
        eprintln!("OPEN_COURSE_SYNC_URL/OC_LIVE_TOKEN unset, skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db = temp_db(&dir).await;
    let revision = client.pull(&db, "no-such-pair").await.unwrap();
    assert_eq!(revision, 0);
    assert!(db.curriculum().read_all().await.unwrap().topics.is_empty());
}
