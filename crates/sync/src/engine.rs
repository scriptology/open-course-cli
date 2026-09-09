//! Sync engine: pushing the local outbox and pulling remote changes with
//! last-writer-wins application.

use open_course_db::Database;
use open_course_db::outbox::OutboxEntry;

use crate::client::{SyncClient, check_status};
use crate::error::{PushError, SyncError};
use crate::protocol::{
    Change, ConflictBody, PullResponse, PushRequest, PushResponse, entity_is_form,
    entity_is_learning_item, entity_is_lemma, entity_to_wire, op_is_delete, op_is_tombstone_reset,
    op_is_upsert, op_to_wire,
};

/// How far past the current time a local `updated_at` may be before it is
/// considered "in the future" (broken clock) and loses to the incoming row.
const FUTURE_SKEW: chrono::Duration = chrono::Duration::minutes(5);

impl SyncClient {
    /// Pushes all pending outbox entries. On success the confirmed entries
    /// are removed from the outbox and the server revision is returned.
    /// On 409 the outbox is left intact and
    /// `PushError::CurriculumConflict` carries the server's canonical
    /// curriculum. Network/5xx failures are retried with backoff; the
    /// outbox is only trimmed after a confirmed push, so a repeated push
    /// of the same operations is safe.
    ///
    /// Note: a successful push does NOT advance `last_pulled_seq` — the
    /// next pull re-receives our own changes (echo) and they no-op via
    /// last-writer-wins, while no remote change can be skipped.
    pub async fn push(&self, db: &Database, pair_id: &str) -> Result<i64, PushError> {
        self.push_inner(db, pair_id, false).await
    }

    pub(crate) async fn push_inner(
        &self,
        db: &Database,
        pair_id: &str,
        force: bool,
    ) -> Result<i64, PushError> {
        let entries = db.outbox().read_all().await.map_err(SyncError::from)?;
        let base_revision = db
            .metadata()
            .last_pulled_seq()
            .await
            .map_err(SyncError::from)?;
        if entries.is_empty() && !force {
            return Ok(base_revision);
        }
        let changes = entries
            .iter()
            .map(protocol_change)
            .collect::<Result<Vec<_>, _>>()?;
        let max_seq = entries.iter().map(|e| e.seq).max().unwrap_or(0);
        let request = PushRequest {
            pair_id: pair_id.to_string(),
            base_revision,
            changes,
            force_curriculum: force.then_some(true),
        };

        let resp = self
            .send_with_retry(|| {
                self.authorized(
                    self.http_ref()
                        .post(self.url("/v1/sync/push"))
                        .json(&request),
                )
            })
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SyncError::Unauthorized.into());
        }
        if status == reqwest::StatusCode::CONFLICT {
            let conflict: ConflictBody = resp.json().await.map_err(SyncError::from)?;
            return Err(PushError::CurriculumConflict(conflict.canonical));
        }
        if !status.is_success() {
            let body = resp.text().await.map_err(SyncError::from)?;
            return Err(SyncError::Server(format!("push failed with {status}: {body}")).into());
        }
        let pushed: PushResponse = resp.json().await.map_err(SyncError::from)?;
        db.outbox()
            .delete_through(max_seq)
            .await
            .map_err(SyncError::from)?;
        Ok(pushed.revision)
    }

    /// Pulls remote changes since `last_pulled_seq` and applies them
    /// (last-writer-wins per row), then advances `last_pulled_seq` to the
    /// server revision. Applying the same pull twice is a no-op.
    pub async fn pull(&self, db: &Database, pair_id: &str) -> Result<i64, SyncError> {
        self.pull_with(db, pair_id, self.http_ref()).await
    }

    /// Pull with the short pull-on-start timeout: sync must never delay
    /// application startup.
    pub async fn pull_with_timeout(&self, db: &Database, pair_id: &str) -> Result<i64, SyncError> {
        self.pull_with(db, pair_id, self.http_short_ref()).await
    }

    async fn pull_with(
        &self,
        db: &Database,
        pair_id: &str,
        http: &reqwest::Client,
    ) -> Result<i64, SyncError> {
        let since = db.metadata().last_pulled_seq().await?;
        let resp = self
            .authorized(http.get(self.url("/v1/sync/pull")))
            .query(&[("pairId", pair_id), ("since", since.to_string().as_str())])
            .send()
            .await?;
        let resp = check_status(resp).await?;
        let pull: PullResponse = resp.json().await?;

        apply_pull(db, &pull).await?;
        db.metadata().set_last_pulled_seq(pull.revision).await?;
        Ok(pull.revision)
    }
}

fn protocol_change(entry: &OutboxEntry) -> Result<Change, SyncError> {
    let payload = if entry.payload.is_empty() {
        None
    } else {
        Some(serde_json::from_str(&entry.payload)?)
    };
    Ok(Change {
        seq: entry.seq,
        op: op_to_wire(&entry.op).to_string(),
        entity: entity_to_wire(&entry.entity).to_string(),
        entity_id: entry.entity_id.clone(),
        payload,
        updated_at: Some(entry.created_at.clone()),
    })
}

/// Applies a pull response: a server-side reset first, then the changes in
/// seq order.
async fn apply_pull(db: &Database, pull: &PullResponse) -> Result<(), SyncError> {
    if let Some(reset_at) = &pull.reset_at {
        reset_all(db).await?;
        db.metadata()
            .set(open_course_db::metadata::KEY_RESET_AT, reset_at)
            .await?;
    }

    // Each table is read ONCE into a cache instead of re-reading it for
    // every change: a first full pull replays thousands of changes, and a
    // full table scan per change is quadratic — on real datasets it looked
    // like an infinite hang.
    let mut caches = PullCaches::load(db).await?;
    let mut pending = PendingWrites::default();
    let mut changes: Vec<&Change> = pull.changes.iter().collect();
    changes.sort_by_key(|c| c.seq);
    for change in changes {
        if op_is_tombstone_reset(&change.op) {
            // Deletes and resets must not overtake buffered upserts.
            pending.flush(db).await?;
            apply_tombstone_reset(db, change, &mut caches).await?;
        } else if op_is_delete(&change.op) {
            pending.flush(db).await?;
            apply_delete(db, change, &mut caches).await?;
        } else if op_is_upsert(&change.op) {
            buffer_upsert(change, &mut caches, &mut pending)?;
        } else {
            return Err(SyncError::Protocol(format!("unknown op: {}", change.op)));
        }
    }
    pending.flush(db).await?;
    Ok(())
}

/// Winning rows buffered while replaying a pull feed, written in bulk at
/// flush points (before any delete/reset and at the end). Per-row writes
/// cost two Lance commits each — batching turns a first full pull from
/// hours into seconds.
#[derive(Default)]
struct PendingWrites {
    topics: Vec<open_course_core::curriculum::Topic>,
    progress: Vec<open_course_core::progress::ProgressTopic>,
    sessions: Vec<open_course_core::history::SessionSummary>,
    learning_items: Vec<open_course_core::learning_items::LearningItem>,
    lemmas: Vec<open_course_core::vocabulary::Lemma>,
    forms: Vec<open_course_core::vocabulary::Form>,
    metadata: Vec<(String, String)>,
}

impl PendingWrites {
    async fn flush(&mut self, db: &Database) -> Result<(), SyncError> {
        // The feed can update one row several times (e.g. the echo of our
        // own push followed by another device's edit); a bulk insert must
        // contain only the last version of each id, or rows duplicate.
        keep_last_by_id(&mut self.topics, |t| &t.id);
        keep_last_by_id(&mut self.progress, |p| &p.topic_id);
        keep_last_by_id(&mut self.learning_items, |i| &i.id);
        keep_last_by_id(&mut self.lemmas, |l| &l.id);
        keep_last_by_id(&mut self.forms, |f| &f.id);
        db.curriculum()
            .upsert_many_with_timestamps(&self.topics)
            .await?;
        self.topics.clear();
        db.progress()
            .upsert_many_with_timestamps(&self.progress)
            .await?;
        self.progress.clear();
        db.history()
            .append_many_with_timestamps(&self.sessions)
            .await?;
        self.sessions.clear();
        db.learning_items()
            .upsert_many_with_timestamps(&self.learning_items)
            .await?;
        self.learning_items.clear();
        db.lemmas()
            .upsert_many_with_timestamps(&self.lemmas)
            .await?;
        self.lemmas.clear();
        db.forms().upsert_many_with_timestamps(&self.forms).await?;
        self.forms.clear();
        for (key, value) in std::mem::take(&mut self.metadata) {
            db.metadata().set(&key, &value).await?;
        }
        Ok(())
    }
}

/// Drops all but the last occurrence of each id, preserving order.
fn keep_last_by_id<T>(rows: &mut Vec<T>, id: impl Fn(&T) -> &str) {
    let mut seen = std::collections::HashSet::new();
    let mut keep = vec![true; rows.len()];
    for (i, row) in rows.iter().enumerate().rev() {
        if !seen.insert(id(row).to_string()) {
            keep[i] = false;
        }
    }
    let mut i = 0;
    rows.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
}

/// Local rows preloaded for one pull application, keyed for O(1) lookups.
#[derive(Default)]
struct PullCaches {
    topics: std::collections::HashMap<String, open_course_core::curriculum::Topic>,
    progress: std::collections::HashMap<String, open_course_core::progress::ProgressTopic>,
    sessions: std::collections::HashSet<String>,
    learning_items:
        std::collections::HashMap<String, open_course_core::learning_items::LearningItem>,
    lemmas: std::collections::HashMap<String, open_course_core::vocabulary::Lemma>,
    forms: std::collections::HashMap<String, open_course_core::vocabulary::Form>,
}

impl PullCaches {
    async fn load(db: &Database) -> Result<Self, SyncError> {
        Ok(Self {
            topics: db
                .curriculum()
                .read_all()
                .await?
                .topics
                .into_iter()
                .map(|t| (t.id.clone(), t))
                .collect(),
            progress: db
                .progress()
                .read_all()
                .await?
                .topics
                .into_iter()
                .map(|p| (p.topic_id.clone(), p))
                .collect(),
            sessions: db
                .history()
                .read_all()
                .await?
                .into_iter()
                .map(|s| s.id)
                .collect(),
            learning_items: db
                .learning_items()
                .read_all()
                .await?
                .into_iter()
                .map(|i| (i.id.clone(), i))
                .collect(),
            lemmas: db
                .lemmas()
                .read_all()
                .await?
                .into_iter()
                .map(|l| (l.id.clone(), l))
                .collect(),
            forms: db
                .forms()
                .read_all()
                .await?
                .into_iter()
                .map(|f| (f.id.clone(), f))
                .collect(),
        })
    }
}

async fn reset_all(db: &Database) -> Result<(), SyncError> {
    db.curriculum().reset().await?;
    db.progress().reset().await?;
    db.history().reset().await?;
    db.learning_items().reset().await?;
    db.lemmas().reset().await?;
    db.forms().reset().await?;
    Ok(())
}

/// Decides an upsert by last-writer-wins against the preloaded caches and
/// buffers the winner for the next bulk flush. Pure decision logic: no I/O.
fn buffer_upsert(
    change: &Change,
    caches: &mut PullCaches,
    pending: &mut PendingWrites,
) -> Result<(), SyncError> {
    let Some(payload) = &change.payload else {
        return Err(SyncError::Protocol(format!(
            "upsert without payload for {} {}",
            change.entity, change.entity_id
        )));
    };
    match change.entity.as_str() {
        "topic" => {
            let incoming: open_course_core::curriculum::Topic =
                serde_json::from_value(payload.clone())?;
            let local_updated = caches
                .topics
                .get(&change.entity_id)
                .and_then(|t| t.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                caches.topics.insert(incoming.id.clone(), incoming.clone());
                pending.topics.push(incoming);
            }
        }
        "progress" => {
            let incoming: open_course_core::progress::ProgressTopic =
                serde_json::from_value(payload.clone())?;
            let local_updated = caches
                .progress
                .get(&change.entity_id)
                .and_then(|t| t.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                caches
                    .progress
                    .insert(incoming.topic_id.clone(), incoming.clone());
                pending.progress.push(incoming);
            }
        }
        "session" => {
            let incoming: open_course_core::history::SessionSummary =
                serde_json::from_value(payload.clone())?;
            // Sessions are append-only with unique ids; a duplicate pull is
            // a no-op.
            if caches.sessions.insert(incoming.id.clone()) {
                pending.sessions.push(incoming);
            }
        }
        entity if entity_is_learning_item(entity) => {
            let incoming: open_course_core::learning_items::LearningItem =
                serde_json::from_value(payload.clone())?;
            let local_updated = caches
                .learning_items
                .get(&change.entity_id)
                .and_then(|i| i.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                caches
                    .learning_items
                    .insert(incoming.id.clone(), incoming.clone());
                pending.learning_items.push(incoming);
            }
        }
        entity if entity_is_lemma(entity) => {
            let incoming: open_course_core::vocabulary::Lemma =
                serde_json::from_value(payload.clone())?;
            let local_updated = caches
                .lemmas
                .get(&change.entity_id)
                .and_then(|l| l.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                caches.lemmas.insert(incoming.id.clone(), incoming.clone());
                pending.lemmas.push(incoming);
            }
        }
        entity if entity_is_form(entity) => {
            let incoming: open_course_core::vocabulary::Form =
                serde_json::from_value(payload.clone())?;
            let local_updated = caches
                .forms
                .get(&change.entity_id)
                .and_then(|f| f.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                caches.forms.insert(incoming.id.clone(), incoming.clone());
                pending.forms.push(incoming);
            }
        }
        "metadata" => {
            // Metadata payload shape: { "key": ..., "value": ... }.
            let key = payload
                .get("key")
                .and_then(|k| k.as_str())
                .ok_or_else(|| SyncError::Protocol("metadata upsert without key".to_string()))?;
            let value = payload
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            pending.metadata.push((key.to_string(), value.to_string()));
        }
        _ => {
            // Forward compatibility: an entity introduced by a newer client
            // is skipped instead of failing the whole pull, so one unknown
            // entity cannot wedge sync forever (the cursor still advances).
        }
    }
    Ok(())
}

/// Last-writer-wins per row: the incoming row wins when the local row or
/// its timestamp is missing, when it is at least as new (ties go to the
/// incoming row — it carries the higher server revision), or when the
/// local timestamp is implausibly far in the future.
fn incoming_wins(local_updated: Option<&str>, incoming_updated: Option<&str>) -> bool {
    match (local_updated, incoming_updated) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(local), Some(incoming)) => incoming >= local || local > future_threshold().as_str(),
    }
}

fn future_threshold() -> String {
    (chrono::Utc::now() + FUTURE_SKEW).to_rfc3339()
}

async fn apply_delete(
    db: &Database,
    change: &Change,
    caches: &mut PullCaches,
) -> Result<(), SyncError> {
    match change.entity.as_str() {
        "topic" => {
            db.curriculum()
                .delete_by_topic_id(&change.entity_id)
                .await?;
            caches.topics.remove(&change.entity_id);
        }
        "progress" => {
            db.progress().delete_by_topic_id(&change.entity_id).await?;
            caches.progress.remove(&change.entity_id);
        }
        entity if entity_is_learning_item(entity) => {
            db.learning_items().delete_by_id(&change.entity_id).await?;
            caches.learning_items.remove(&change.entity_id);
        }
        entity if entity_is_lemma(entity) => {
            db.lemmas().delete_by_id(&change.entity_id).await?;
            caches.lemmas.remove(&change.entity_id);
        }
        entity if entity_is_form(entity) => {
            db.forms().delete_by_id(&change.entity_id).await?;
            caches.forms.remove(&change.entity_id);
        }
        // Sessions are append-only and never deleted; metadata keys have no
        // delete operation.
        _ => {}
    }
    Ok(())
}

async fn apply_tombstone_reset(
    db: &Database,
    change: &Change,
    caches: &mut PullCaches,
) -> Result<(), SyncError> {
    let reset_at = change
        .updated_at
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    match change.entity.as_str() {
        "topic" => {
            db.curriculum().reset().await?;
            caches.topics.clear();
        }
        "progress" => {
            db.progress().reset().await?;
            caches.progress.clear();
        }
        "session" => {
            db.history().reset().await?;
            caches.sessions.clear();
        }
        entity if entity_is_learning_item(entity) => {
            db.learning_items().reset().await?;
            caches.learning_items.clear();
        }
        entity if entity_is_lemma(entity) => {
            db.lemmas().reset().await?;
            caches.lemmas.clear();
        }
        entity if entity_is_form(entity) => {
            db.forms().reset().await?;
            caches.forms.clear();
        }
        // An explicit "*" entity still wipes all synced tables.
        "*" => {
            reset_all(db).await?;
            *caches = PullCaches::default();
        }
        // Forward compatibility: a tombstone reset for an entity this client
        // does not know is a no-op — resetting everything here would wipe
        // unrelated local data on every new entity rollout. The reset
        // marker is not recorded either: nothing was actually reset.
        _ => return Ok(()),
    }
    db.metadata()
        .set(open_course_db::metadata::KEY_RESET_AT, &reset_at)
        .await?;
    Ok(())
}
