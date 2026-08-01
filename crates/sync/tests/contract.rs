//! Contract tests for the sync protocol against a stateful in-memory mock
//! server (axum on localhost). No real network is involved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tempfile::TempDir;

use open_course_core::curriculum::Topic;
use open_course_db::Database;
use open_course_db::outbox::{ENTITY_LEARNING_ITEM, ENTITY_TOPIC, OP_TOMBSTONE_RESET, OP_UPSERT};
use open_course_sync::{
    Change, CurriculumPayload, DeviceCodeResponse, MeResponse, PollResult, PullResponse,
    PushRequest, PushResponse, SyncClient, SyncError, TokenBackend, TokenSet, TokenStore,
};

const TEST_TOKEN: &str = "test-token";

// ---------------------------------------------------------------------------
// Stateful mock server
// ---------------------------------------------------------------------------

type Shared = Arc<Mutex<ServerState>>;

#[derive(Default)]
struct ServerState {
    // Device flow.
    device_authorized_after: usize,
    device_polls: usize,
    device_expired: bool,
    // Sync.
    revision: i64,
    changes: Vec<Change>,
    fail_push_times: usize,
    /// The server's canonical curriculum, set by the first topic push and
    /// replaced by a `forceCurriculum` push.
    canonical: Option<CurriculumPayload>,
    reset_at: Option<String>,
    pull_delay_ms: u64,
    last_push: Option<PushRequest>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            // Not authorized by default.
            device_authorized_after: usize::MAX,
            ..Default::default()
        }
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(format!("Bearer {TEST_TOKEN}").as_str())
}

async fn auth_device() -> Json<DeviceCodeResponse> {
    Json(DeviceCodeResponse {
        device_code: "dc-1".to_string(),
        user_code: "UC-42".to_string(),
        verification_url: "https://example.test/activate".to_string(),
        expires_in: 600,
        interval: 0,
    })
}

async fn auth_poll(State(state): State<Shared>) -> Response {
    let mut st = state.lock().unwrap();
    st.device_polls += 1;
    if st.device_expired {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "expired_token" })),
        )
            .into_response();
    }
    if st.device_polls <= st.device_authorized_after {
        return (
            StatusCode::from_u16(428).unwrap(),
            Json(serde_json::json!({ "error": "authorization_pending" })),
        )
            .into_response();
    }
    Json(TokenSet {
        access_token: TEST_TOKEN.to_string(),
        refresh_token: Some("refresh-1".to_string()),
        device_id: "dev-1".to_string(),
        user_email: Some("user@example.test".to_string()),
    })
    .into_response()
}

async fn me_handler(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(MeResponse {
        email: "user@example.test".to_string(),
        subscription_status: "active".to_string(),
    })
    .into_response()
}

async fn sync_push(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<PushRequest>,
) -> Response {
    let mut st = state.lock().unwrap();
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if st.fail_push_times > 0 {
        st.fail_push_times -= 1;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    st.last_push = Some(req.clone());
    let force = req.force_curriculum == Some(true);
    let topic_upserts: Vec<Topic> = req
        .changes
        .iter()
        .filter(|c| c.entity == "topic" && c.op == "upsert")
        .filter_map(|c| {
            c.payload
                .as_ref()
                .and_then(|p| serde_json::from_value::<Topic>(p.clone()).ok())
        })
        .collect();
    if !topic_upserts.is_empty() {
        // A curriculum push conflicts only from a device that never pulled
        // the canon (base_revision 0) — bound devices push topic updates
        // freely, and a forced push replaces the canon.
        if st.canonical.is_some() && req.base_revision == 0 && !force {
            let canonical = st.canonical.clone().unwrap();
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "canonical": canonical })),
            )
                .into_response();
        }
        // First push marks the canon; a forced push replaces it.
        let mut topics = if force {
            Vec::new()
        } else {
            st.canonical
                .as_ref()
                .map(|c| c.topics.clone())
                .unwrap_or_default()
        };
        for topic in topic_upserts {
            match topics.iter_mut().find(|t| t.id == topic.id) {
                Some(existing) => *existing = topic,
                None => topics.push(topic),
            }
        }
        let version = topics.iter().map(|t| t.version).max().unwrap_or(1);
        st.canonical = Some(CurriculumPayload {
            revision: st.revision + req.changes.len() as i64,
            version,
            topics,
        });
    }
    for change in req.changes {
        st.revision += 1;
        let mut change = change;
        change.seq = st.revision;
        st.changes.push(change);
    }
    Json(PushResponse {
        revision: st.revision,
    })
    .into_response()
}

async fn sync_pull(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (delay, revision, changes, reset_at) = {
        let st = state.lock().unwrap();
        let since: i64 = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
        let changes: Vec<Change> = st
            .changes
            .iter()
            .filter(|c| c.seq > since)
            .cloned()
            .collect();
        (st.pull_delay_ms, st.revision, changes, st.reset_at.clone())
    };
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    Json(PullResponse {
        revision,
        changes,
        reset_at,
    })
    .into_response()
}

async fn start_mock() -> (Shared, String) {
    let state: Shared = Arc::new(Mutex::new(ServerState::new()));
    let app = Router::new()
        .route("/auth/device", post(auth_device))
        .route("/auth/device/poll", post(auth_poll))
        .route("/v1/me", get(me_handler))
        .route("/v1/sync/push", post(sync_push))
        .route("/v1/sync/pull", get(sync_pull))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, format!("http://{addr}"))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn temp_db() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    (dir, db)
}

fn client(base_url: &str) -> SyncClient {
    SyncClient::new(base_url)
        .unwrap()
        .with_access_token(TEST_TOKEN)
}

fn topic(id: &str, name: &str, updated_at: Option<&str>) -> Topic {
    Topic {
        id: id.to_string(),
        name: name.to_string(),
        difficulty: "beginner".to_string(),
        level: Some("A1".to_string()),
        target_lang: "es".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
        updated_at: updated_at.map(|s| s.to_string()),
        ..Default::default()
    }
}

async fn outbox_upsert_topic(db: &Database, t: &Topic) {
    db.outbox()
        .append(
            OP_UPSERT,
            ENTITY_TOPIC,
            &t.id,
            &serde_json::to_string(t).unwrap(),
        )
        .await
        .unwrap();
}

fn wire_upsert_topic(seq: i64, t: &Topic) -> Change {
    Change {
        seq,
        op: "upsert".to_string(),
        entity: "topic".to_string(),
        entity_id: t.id.clone(),
        payload: Some(serde_json::to_value(t).unwrap()),
        updated_at: t.updated_at.clone(),
    }
}

// ---------------------------------------------------------------------------
// Device flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn device_flow_pending_then_authorized() {
    let (state, base) = start_mock().await;
    state.lock().unwrap().device_authorized_after = 2;
    let client = client(&base);

    let device = client.start_device_flow().await.unwrap();
    assert_eq!(device.device_code, "dc-1");
    assert_eq!(device.user_code, "UC-42");

    assert!(matches!(
        client.poll_device_flow(&device.device_code).await.unwrap(),
        PollResult::Pending
    ));
    assert!(matches!(
        client.poll_device_flow(&device.device_code).await.unwrap(),
        PollResult::Pending
    ));
    match client.poll_device_flow(&device.device_code).await.unwrap() {
        PollResult::Authorized(tokens) => {
            assert_eq!(tokens.access_token, TEST_TOKEN);
            assert_eq!(tokens.device_id, "dev-1");
        }
        other => panic!("expected Authorized, got {other:?}"),
    }
}

#[tokio::test]
async fn device_flow_expired() {
    let (state, base) = start_mock().await;
    state.lock().unwrap().device_expired = true;
    let client = client(&base);
    assert!(matches!(
        client.poll_device_flow("dc-1").await.unwrap(),
        PollResult::Expired
    ));
}

#[tokio::test]
async fn me_returns_profile_and_rejects_bad_token() {
    let (_state, base) = start_mock().await;
    let me = client(&base).me().await.unwrap();
    assert_eq!(me.email, "user@example.test");
    assert_eq!(me.subscription_status, "active");

    let anon = SyncClient::new(&base).unwrap();
    assert!(matches!(anon.me().await, Err(SyncError::Unauthorized)));
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_sends_outbox_as_changes_and_clears_confirmed() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    let t = topic("t1", "Greetings", Some("2025-01-01T00:00:00Z"));
    outbox_upsert_topic(&db, &t).await;
    db.outbox()
        .append(
            OP_TOMBSTONE_RESET,
            ENTITY_LEARNING_ITEM,
            "*",
            "{\"reset_at\":\"2025-01-02T00:00:00Z\"}",
        )
        .await
        .unwrap();

    let revision = client(&base).push(&db, "ru-es").await.unwrap();
    assert_eq!(revision, 2);
    assert_eq!(db.outbox().len().await.unwrap(), 0);

    // The wire format uses camelCase op/entity names.
    let st = state.lock().unwrap();
    let pushed = st.last_push.as_ref().unwrap();
    assert_eq!(pushed.pair_id, "ru-es");
    assert_eq!(pushed.changes.len(), 2);
    assert_eq!(pushed.changes[0].entity, "topic");
    assert_eq!(pushed.changes[1].entity, "learningItem");
    assert_eq!(pushed.changes[1].op, "tombstoneReset");
}

#[tokio::test]
async fn push_retries_on_5xx_and_keeps_outbox_on_failure() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    outbox_upsert_topic(&db, &topic("t1", "Greetings", None)).await;

    // Every attempt fails: error, outbox intact.
    state.lock().unwrap().fail_push_times = 5;
    let result = client(&base).push(&db, "ru-es").await;
    assert!(result.is_err());
    assert_eq!(db.outbox().len().await.unwrap(), 1);

    // Two failures then success: retried through, outbox cleared.
    state.lock().unwrap().fail_push_times = 2;
    let revision = client(&base).push(&db, "ru-es").await.unwrap();
    assert!(revision > 0);
    assert_eq!(db.outbox().len().await.unwrap(), 0);
}

#[tokio::test]
async fn push_conflict_returns_canonical_and_keeps_outbox() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    outbox_upsert_topic(&db, &topic("t1", "Greetings", None)).await;
    state.lock().unwrap().canonical = Some(CurriculumPayload {
        revision: 42,
        version: 3,
        topics: vec![topic("server-t", "Canonical", Some("2025-03-01T00:00:00Z"))],
    });

    match client(&base).push(&db, "ru-es").await {
        Err(open_course_sync::PushError::CurriculumConflict(canonical)) => {
            assert_eq!(canonical.revision, 42);
            assert_eq!(canonical.topics[0].id, "server-t");
        }
        other => panic!("expected CurriculumConflict, got {other:?}"),
    }
    assert_eq!(db.outbox().len().await.unwrap(), 1);
}

#[tokio::test]
async fn push_rejects_unauthorized() {
    let (_state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    outbox_upsert_topic(&db, &topic("t1", "Greetings", None)).await;
    let anon = SyncClient::new(&base).unwrap();
    let result = anon.push(&db, "ru-es").await;
    assert!(matches!(
        result,
        Err(open_course_sync::PushError::Sync(SyncError::Unauthorized))
    ));
    assert_eq!(db.outbox().len().await.unwrap(), 1);
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

fn seed_changes(state: &Shared, changes: Vec<Change>) {
    let mut st = state.lock().unwrap();
    for change in changes {
        st.revision = st.revision.max(change.seq);
        st.changes.push(change);
    }
}

#[tokio::test]
async fn pull_applies_delta_and_advances_cursor() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    seed_changes(
        &state,
        vec![
            wire_upsert_topic(1, &topic("t1", "Greetings", Some("2025-01-01T00:00:00Z"))),
            Change {
                seq: 2,
                op: "upsert".to_string(),
                entity: "session".to_string(),
                entity_id: "s1".to_string(),
                payload: Some(
                    serde_json::to_value(open_course_core::history::SessionSummary {
                        id: "s1".to_string(),
                        date: "2025-01-01T00:00:00Z".to_string(),
                        avg_target_score: 80.0,
                        updated_at: Some("2025-01-01T00:00:00Z".to_string()),
                        ..Default::default()
                    })
                    .unwrap(),
                ),
                updated_at: Some("2025-01-01T00:00:00Z".to_string()),
            },
        ],
    );

    let revision = client(&base).pull(&db, "ru-es").await.unwrap();
    assert_eq!(revision, 2);
    assert_eq!(db.metadata().last_pulled_seq().await.unwrap(), 2);

    let curriculum = db.curriculum().read_all().await.unwrap();
    assert_eq!(curriculum.topics.len(), 1);
    assert_eq!(curriculum.topics[0].name, "Greetings");
    // Timestamps preserved from the server.
    assert_eq!(
        curriculum.topics[0].updated_at.as_deref(),
        Some("2025-01-01T00:00:00Z")
    );
    assert_eq!(db.history().read_all().await.unwrap().len(), 1);

    // Second pull: nothing new, and the session is not duplicated.
    let revision = client(&base).pull(&db, "ru-es").await.unwrap();
    assert_eq!(revision, 2);
    assert_eq!(db.history().read_all().await.unwrap().len(), 1);
    assert_eq!(db.curriculum().read_all().await.unwrap().topics.len(), 1);
}

#[tokio::test]
async fn pull_lww_stale_incoming_does_not_overwrite_newer_local() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert_with_timestamps(&topic("t1", "Local new", Some("2025-06-01T00:00:00Z")))
        .await
        .unwrap();
    seed_changes(
        &state,
        vec![wire_upsert_topic(
            1,
            &topic("t1", "Remote stale", Some("2025-01-01T00:00:00Z")),
        )],
    );

    client(&base).pull(&db, "ru-es").await.unwrap();
    let curriculum = db.curriculum().read_all().await.unwrap();
    assert_eq!(curriculum.topics[0].name, "Local new");
}

#[tokio::test]
async fn pull_lww_tie_goes_to_incoming() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert_with_timestamps(&topic("t1", "Local", Some("2025-01-01T00:00:00Z")))
        .await
        .unwrap();
    seed_changes(
        &state,
        vec![wire_upsert_topic(
            1,
            &topic("t1", "Remote", Some("2025-01-01T00:00:00Z")),
        )],
    );

    client(&base).pull(&db, "ru-es").await.unwrap();
    assert_eq!(
        db.curriculum().read_all().await.unwrap().topics[0].name,
        "Remote"
    );
}

#[tokio::test]
async fn pull_lww_local_timestamp_in_future_loses() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert_with_timestamps(&topic("t1", "Local", Some("2999-01-01T00:00:00Z")))
        .await
        .unwrap();
    seed_changes(
        &state,
        vec![wire_upsert_topic(
            1,
            &topic("t1", "Remote", Some("2025-01-01T00:00:00Z")),
        )],
    );

    client(&base).pull(&db, "ru-es").await.unwrap();
    let curriculum = db.curriculum().read_all().await.unwrap();
    assert_eq!(curriculum.topics[0].name, "Remote");
    assert_eq!(
        curriculum.topics[0].updated_at.as_deref(),
        Some("2025-01-01T00:00:00Z")
    );
}

#[tokio::test]
async fn pull_applies_delete_and_tombstone_reset() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert_with_timestamps(&topic("t1", "Greetings", Some("2025-01-01T00:00:00Z")))
        .await
        .unwrap();
    db.learning_items()
        .upsert_with_timestamps(&open_course_core::learning_items::LearningItem {
            id: "i1".to_string(),
            name: "Caro vs Rico".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

    seed_changes(
        &state,
        vec![
            Change {
                seq: 1,
                op: "delete".to_string(),
                entity: "topic".to_string(),
                entity_id: "t1".to_string(),
                payload: None,
                updated_at: Some("2025-02-01T00:00:00Z".to_string()),
            },
            Change {
                seq: 2,
                op: "tombstoneReset".to_string(),
                entity: "learningItem".to_string(),
                entity_id: "*".to_string(),
                payload: None,
                updated_at: Some("2025-02-02T00:00:00Z".to_string()),
            },
        ],
    );

    client(&base).pull(&db, "ru-es").await.unwrap();
    assert!(db.curriculum().read_all().await.unwrap().topics.is_empty());
    assert!(db.learning_items().read_all().await.unwrap().is_empty());
    assert_eq!(
        db.metadata()
            .get(open_course_db::metadata::KEY_RESET_AT)
            .await
            .unwrap()
            .as_deref(),
        Some("2025-02-02T00:00:00Z")
    );
}

#[tokio::test]
async fn pull_top_level_reset_wipes_before_applying() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert_with_timestamps(&topic("old", "Old", Some("2025-01-01T00:00:00Z")))
        .await
        .unwrap();
    state.lock().unwrap().reset_at = Some("2025-03-01T00:00:00Z".to_string());
    seed_changes(
        &state,
        vec![wire_upsert_topic(
            1,
            &topic("fresh", "Fresh", Some("2025-03-01T00:00:00Z")),
        )],
    );

    client(&base).pull(&db, "ru-es").await.unwrap();
    let curriculum = db.curriculum().read_all().await.unwrap();
    assert_eq!(curriculum.topics.len(), 1);
    assert_eq!(curriculum.topics[0].id, "fresh");
}

#[tokio::test]
async fn pull_with_timeout_fails_fast_on_slow_server() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    state.lock().unwrap().pull_delay_ms = 10_000;

    let start = std::time::Instant::now();
    let result = client(&base).pull_with_timeout(&db, "ru-es").await;
    assert!(result.is_err());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(8),
        "short-timeout pull should fail fast, took {:?}",
        start.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Roundtrip: two clients against one server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn roundtrip_two_clients() {
    let (state, base) = start_mock().await;
    let (_dir_a, db_a) = temp_db().await;
    let (_dir_b, db_b) = temp_db().await;

    // A creates a topic and pushes it.
    let t = topic("t1", "Greetings", None);
    db_a.curriculum().upsert(&t).await.unwrap();
    let stored = db_a.curriculum().read_all().await.unwrap();
    let stamped = stored.topics.into_iter().find(|t| t.id == "t1").unwrap();
    outbox_upsert_topic(&db_a, &stamped).await;
    client(&base).push(&db_a, "ru-es").await.unwrap();

    // B pulls it.
    client(&base).pull(&db_b, "ru-es").await.unwrap();
    let b_curriculum = db_b.curriculum().read_all().await.unwrap();
    assert_eq!(b_curriculum.topics.len(), 1);
    assert_eq!(b_curriculum.topics[0].name, "Greetings");
    assert_eq!(b_curriculum.topics[0].updated_at, stamped.updated_at);

    // B renames the topic and pushes; A pulls and LWW applies B's change.
    let mut renamed = b_curriculum.topics[0].clone();
    renamed.name = "Greetings (edited on B)".to_string();
    db_b.curriculum().upsert(&renamed).await.unwrap();
    let stored_b = db_b.curriculum().read_all().await.unwrap();
    let stamped_b = stored_b.topics.into_iter().find(|t| t.id == "t1").unwrap();
    outbox_upsert_topic(&db_b, &stamped_b).await;
    client(&base).push(&db_b, "ru-es").await.unwrap();

    client(&base).pull(&db_a, "ru-es").await.unwrap();
    let a_curriculum = db_a.curriculum().read_all().await.unwrap();
    assert_eq!(a_curriculum.topics.len(), 1);
    assert_eq!(a_curriculum.topics[0].name, "Greetings (edited on B)");

    // A resets its data; the tombstone_reset propagates to B.
    db_a.outbox()
        .append(
            OP_TOMBSTONE_RESET,
            ENTITY_TOPIC,
            "*",
            "{\"reset_at\":\"2030-01-01T00:00:00Z\"}",
        )
        .await
        .unwrap();
    client(&base).push(&db_a, "ru-es").await.unwrap();
    client(&base).pull(&db_b, "ru-es").await.unwrap();
    assert!(
        db_b.curriculum()
            .read_all()
            .await
            .unwrap()
            .topics
            .is_empty()
    );

    // Sanity: the server saw all three pushes.
    assert_eq!(state.lock().unwrap().revision, 3);
}

// ---------------------------------------------------------------------------
// Token store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_store_file_backend_roundtrip() {
    let dir = TempDir::new().unwrap();
    let store = TokenStore::with_backend(dir.path().to_path_buf(), TokenBackend::File);
    assert_eq!(store.backend(), TokenBackend::File);

    assert!(store.load().await.unwrap().is_none());

    let tokens = TokenSet {
        access_token: "access-1".to_string(),
        refresh_token: Some("refresh-1".to_string()),
        device_id: "dev-1".to_string(),
        user_email: Some("user@example.test".to_string()),
    };
    store.save(&tokens).await.unwrap();

    let loaded = store.load().await.unwrap().unwrap();
    assert_eq!(loaded.access_token, "access-1");
    assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-1"));
    assert_eq!(loaded.device_id, "dev-1");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "auth.json must be owner-only");
    }

    store.delete().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    // Deleting twice is fine.
    store.delete().await.unwrap();
}

// ---------------------------------------------------------------------------
// First-bind conflict resolution: adopt / replace / progress merge
// ---------------------------------------------------------------------------

use open_course_core::progress::ProgressTopic;
use open_course_db::outbox::ENTITY_PROGRESS;
use open_course_sync::{BindScenario, ProgressMerge};

async fn outbox_upsert_progress(db: &Database, p: &ProgressTopic) {
    db.outbox()
        .append(
            OP_UPSERT,
            ENTITY_PROGRESS,
            &p.topic_id,
            &serde_json::to_string(p).unwrap(),
        )
        .await
        .unwrap();
}

/// Machine A pushes its curriculum, marking the server's canonical version.
async fn seed_cloud(state: &Shared, base: &str, pair: &str) -> Database {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("db");
    // Leak the tempdir: the returned Database outlives this function.
    std::mem::forget(dir);
    let db = Database::connect(&path).await.unwrap();
    let t = topic("cloud-t1", "Cloud topic", None);
    db.curriculum().upsert(&t).await.unwrap();
    let stamped = db
        .curriculum()
        .read_all()
        .await
        .unwrap()
        .topics
        .into_iter()
        .find(|t| t.id == "cloud-t1")
        .unwrap();
    outbox_upsert_topic(&db, &stamped).await;
    client(base).push(&db, pair).await.unwrap();
    assert!(state.lock().unwrap().canonical.is_some());
    db
}

#[tokio::test]
async fn bind_scenarios_fresh_local_and_fresh_cloud() {
    let (state, base) = start_mock().await;
    let (_dir, db) = temp_db().await;
    db.curriculum()
        .upsert(&topic("t1", "Greetings", None))
        .await
        .unwrap();
    assert!(matches!(
        client(&base)
            .first_bind_choices(&db, "ru-es")
            .await
            .unwrap(),
        BindScenario::FreshLocal
    ));

    seed_cloud(&state, &base, "ru-es").await;

    let (_dir2, empty_db) = temp_db().await;
    assert!(matches!(
        client(&base)
            .first_bind_choices(&empty_db, "ru-es")
            .await
            .unwrap(),
        BindScenario::FreshCloud
    ));
}

#[tokio::test]
async fn bind_conflict_then_adopt_tombstones_local_topics_but_keeps_progress() {
    let (state, base) = start_mock().await;
    seed_cloud(&state, &base, "ru-es").await;

    let (_dir, db_b) = temp_db().await;
    db_b.curriculum()
        .upsert(&topic("local-only", "Local only", None))
        .await
        .unwrap();
    db_b.progress()
        .upsert(&ProgressTopic::initial("local-only".to_string(), 40.0))
        .await
        .unwrap();

    let scenario = client(&base)
        .first_bind_choices(&db_b, "ru-es")
        .await
        .unwrap();
    let payload = match scenario {
        BindScenario::Conflict(payload) => payload,
        other => panic!("expected Conflict, got {other:?}"),
    };
    assert!(payload.topics.iter().any(|t| t.id == "cloud-t1"));

    client(&base)
        .adopt_cloud_curriculum(&db_b, "ru-es", &payload, ProgressMerge::Merge)
        .await
        .unwrap();

    let topics = db_b.curriculum().read_all().await.unwrap().topics;
    assert!(topics.iter().any(|t| t.id == "cloud-t1"));
    assert!(
        !topics.iter().any(|t| t.id == "local-only"),
        "local topic missing from the canon is tombstoned"
    );
    // Orphaned progress is preserved (inactive, not deleted).
    let progress = db_b.progress().read_all().await.unwrap();
    assert!(
        progress.topics.iter().any(|p| p.topic_id == "local-only"),
        "orphaned progress must stay in the database"
    );
    assert_eq!(
        db_b.metadata().cloud_curriculum_version().await.unwrap(),
        Some(payload.version)
    );
}

#[tokio::test]
async fn replace_makes_local_curriculum_canonical_and_other_devices_pull_it() {
    let (state, base) = start_mock().await;
    let db_a = seed_cloud(&state, &base, "ru-es").await;

    let (_dir, db_b) = temp_db().await;
    db_b.curriculum()
        .upsert(&topic("b-topic", "B topic", None))
        .await
        .unwrap();

    let scenario = client(&base)
        .first_bind_choices(&db_b, "ru-es")
        .await
        .unwrap();
    assert!(matches!(scenario, BindScenario::Conflict(_)));

    let revision = client(&base)
        .replace_cloud_curriculum(&db_b, "ru-es")
        .await
        .unwrap();
    assert!(revision > 0);

    // The canon is B's curriculum with the version bumped to max(1, 1) + 1.
    {
        let st = state.lock().unwrap();
        let canonical = st.canonical.as_ref().unwrap();
        assert_eq!(canonical.version, 2);
        assert!(canonical.topics.iter().any(|t| t.id == "b-topic"));
        assert!(!canonical.topics.iter().any(|t| t.id == "cloud-t1"));
    }
    assert_eq!(
        db_b.metadata().cloud_curriculum_version().await.unwrap(),
        Some(2)
    );

    // A pulls and receives B's replacement topic.
    client(&base).pull(&db_a, "ru-es").await.unwrap();
    let a_topics = db_a.curriculum().read_all().await.unwrap().topics;
    assert!(a_topics.iter().any(|t| t.id == "b-topic"));
}

#[tokio::test]
async fn adopt_merge_applies_cloud_progress_and_keeps_local_only_rows() {
    let (_state, base) = start_mock().await;
    let (_dir_a, db_a) = temp_db().await;
    let t = topic("t1", "Greetings", None);
    db_a.curriculum().upsert(&t).await.unwrap();
    let stamped = db_a
        .curriculum()
        .read_all()
        .await
        .unwrap()
        .topics
        .into_iter()
        .find(|t| t.id == "t1")
        .unwrap();
    outbox_upsert_topic(&db_a, &stamped).await;
    let cloud_progress = ProgressTopic {
        topic_id: "t1".to_string(),
        score: 80.0,
        mastery: 80.0,
        updated_at: Some("2025-05-01T00:00:00Z".to_string()),
        ..Default::default()
    };
    db_a.progress().upsert(&cloud_progress).await.unwrap();
    outbox_upsert_progress(&db_a, &cloud_progress).await;
    client(&base).push(&db_a, "ru-es").await.unwrap();

    // B: stale local progress (unknown timestamps) for t1 and a row the
    // cloud does not have.
    let (_dir_b, db_b) = temp_db().await;
    db_b.progress()
        .upsert_with_timestamps(&ProgressTopic {
            topic_id: "t1".to_string(),
            score: 50.0,
            mastery: 50.0,
            ..Default::default()
        })
        .await
        .unwrap();
    db_b.progress()
        .upsert_with_timestamps(&ProgressTopic {
            topic_id: "local-extra".to_string(),
            score: 30.0,
            mastery: 30.0,
            ..Default::default()
        })
        .await
        .unwrap();
    db_b.curriculum()
        .upsert(&topic("t1", "Greetings", None))
        .await
        .unwrap();

    let scenario = client(&base)
        .first_bind_choices(&db_b, "ru-es")
        .await
        .unwrap();
    let payload = match scenario {
        BindScenario::Conflict(payload) => payload,
        other => panic!("expected Conflict, got {other:?}"),
    };
    client(&base)
        .adopt_cloud_curriculum(&db_b, "ru-es", &payload, ProgressMerge::Merge)
        .await
        .unwrap();

    let progress = db_b.progress().read_all().await.unwrap();
    let t1 = progress
        .topics
        .iter()
        .find(|p| p.topic_id == "t1")
        .expect("t1 progress present");
    assert_eq!(t1.score, 80.0, "cloud wins over unknown local timestamp");
    assert!(
        progress.topics.iter().any(|p| p.topic_id == "local-extra"),
        "local-only progress is kept for a later push"
    );
}

#[tokio::test]
async fn adopt_start_from_cloud_replaces_progress_without_pushing_deletes() {
    let (state, base) = start_mock().await;
    let (_dir_a, db_a) = temp_db().await;
    let t = topic("t1", "Greetings", None);
    db_a.curriculum().upsert(&t).await.unwrap();
    let stamped = db_a
        .curriculum()
        .read_all()
        .await
        .unwrap()
        .topics
        .into_iter()
        .find(|t| t.id == "t1")
        .unwrap();
    outbox_upsert_topic(&db_a, &stamped).await;
    let cloud_progress = ProgressTopic {
        topic_id: "t1".to_string(),
        score: 80.0,
        mastery: 80.0,
        updated_at: Some("2025-05-01T00:00:00Z".to_string()),
        ..Default::default()
    };
    db_a.progress().upsert(&cloud_progress).await.unwrap();
    outbox_upsert_progress(&db_a, &cloud_progress).await;
    client(&base).push(&db_a, "ru-es").await.unwrap();

    let (_dir_b, db_b) = temp_db().await;
    db_b.curriculum()
        .upsert(&topic("t1", "Greetings", None))
        .await
        .unwrap();
    db_b.progress()
        .upsert(&ProgressTopic::initial("t1".to_string(), 10.0))
        .await
        .unwrap();
    db_b.learning_items()
        .upsert(&open_course_core::learning_items::LearningItem {
            id: "i1".to_string(),
            name: "Caro vs Rico".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

    let scenario = client(&base)
        .first_bind_choices(&db_b, "ru-es")
        .await
        .unwrap();
    let payload = match scenario {
        BindScenario::Conflict(payload) => payload,
        other => panic!("expected Conflict, got {other:?}"),
    };
    client(&base)
        .adopt_cloud_curriculum(&db_b, "ru-es", &payload, ProgressMerge::StartFromCloud)
        .await
        .unwrap();

    // Local progress is replaced by the cloud's.
    let progress = db_b.progress().read_all().await.unwrap();
    let t1 = progress
        .topics
        .iter()
        .find(|p| p.topic_id == "t1")
        .expect("cloud progress applied");
    assert_eq!(t1.score, 80.0);
    assert!(db_b.learning_items().read_all().await.unwrap().is_empty());

    // Critically, the wipe bypassed the outbox: no delete ops ever reach
    // the server.
    let outbox = db_b.outbox().read_all().await.unwrap();
    assert!(
        outbox.iter().all(|e| e.op != "delete"),
        "no delete operations may be queued for the server, got {outbox:?}"
    );
    let revision = client(&base).push(&db_b, "ru-es").await.unwrap();
    assert!(revision > 0);
    let st = state.lock().unwrap();
    let pushed = st.last_push.as_ref().unwrap();
    assert!(
        pushed.changes.iter().all(|c| c.op != "delete"),
        "server must not receive deletes for the discarded local data"
    );
}
