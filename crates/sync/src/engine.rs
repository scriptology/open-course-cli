//! Sync engine: pushing the local outbox and pulling remote changes with
//! last-writer-wins application.

use open_course_db::Database;
use open_course_db::outbox::OutboxEntry;

use crate::client::{SyncClient, check_status};
use crate::error::{PushError, SyncError};
use crate::protocol::{
    Change, ConflictBody, PullResponse, PushRequest, PushResponse, entity_is_learning_item,
    entity_to_wire, op_is_delete, op_is_tombstone_reset, op_is_upsert, op_to_wire,
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

    let mut changes: Vec<&Change> = pull.changes.iter().collect();
    changes.sort_by_key(|c| c.seq);
    for change in changes {
        apply_change(db, change).await?;
    }
    Ok(())
}

async fn reset_all(db: &Database) -> Result<(), SyncError> {
    db.curriculum().reset().await?;
    db.progress().reset().await?;
    db.history().reset().await?;
    db.learning_items().reset().await?;
    Ok(())
}

async fn apply_change(db: &Database, change: &Change) -> Result<(), SyncError> {
    if op_is_upsert(&change.op) {
        apply_upsert(db, change).await
    } else if op_is_delete(&change.op) {
        apply_delete(db, change).await
    } else if op_is_tombstone_reset(&change.op) {
        apply_tombstone_reset(db, change).await
    } else {
        Err(SyncError::Protocol(format!("unknown op: {}", change.op)))
    }
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

async fn apply_upsert(db: &Database, change: &Change) -> Result<(), SyncError> {
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
            let local = db
                .curriculum()
                .read_all()
                .await?
                .topics
                .into_iter()
                .find(|t| t.id == change.entity_id);
            let local_updated = local.as_ref().and_then(|t| t.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                db.curriculum().upsert_with_timestamps(&incoming).await?;
            }
        }
        "progress" => {
            let incoming: open_course_core::progress::ProgressTopic =
                serde_json::from_value(payload.clone())?;
            let local = db.progress().get_by_topic_id(&change.entity_id).await?;
            let local_updated = local.as_ref().and_then(|t| t.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                db.progress().upsert_with_timestamps(&incoming).await?;
            }
        }
        "session" => {
            let incoming: open_course_core::history::SessionSummary =
                serde_json::from_value(payload.clone())?;
            // Sessions are append-only with unique ids; a duplicate pull is
            // a no-op.
            let exists = db
                .history()
                .read_all()
                .await?
                .iter()
                .any(|s| s.id == incoming.id);
            if !exists {
                db.history().append_with_timestamps(&incoming).await?;
            }
        }
        entity if entity_is_learning_item(entity) => {
            let incoming: open_course_core::learning_items::LearningItem =
                serde_json::from_value(payload.clone())?;
            let local = db
                .learning_items()
                .read_all()
                .await?
                .into_iter()
                .find(|i| i.id == change.entity_id);
            let local_updated = local.as_ref().and_then(|i| i.updated_at.as_deref());
            if incoming_wins(local_updated, incoming.updated_at.as_deref()) {
                db.learning_items()
                    .upsert_with_timestamps(&incoming)
                    .await?;
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
            db.metadata().set(key, value).await?;
        }
        other => {
            return Err(SyncError::Protocol(format!("unknown entity: {other}")));
        }
    }
    Ok(())
}

async fn apply_delete(db: &Database, change: &Change) -> Result<(), SyncError> {
    match change.entity.as_str() {
        "topic" => {
            db.curriculum()
                .delete_by_topic_id(&change.entity_id)
                .await?
        }
        "progress" => db.progress().delete_by_topic_id(&change.entity_id).await?,
        entity if entity_is_learning_item(entity) => {
            db.learning_items().delete_by_id(&change.entity_id).await?
        }
        // Sessions are append-only and never deleted; metadata keys have no
        // delete operation.
        _ => {}
    }
    Ok(())
}

async fn apply_tombstone_reset(db: &Database, change: &Change) -> Result<(), SyncError> {
    let reset_at = change
        .updated_at
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    match change.entity.as_str() {
        "topic" => db.curriculum().reset().await?,
        "progress" => db.progress().reset().await?,
        "session" => db.history().reset().await?,
        entity if entity_is_learning_item(entity) => db.learning_items().reset().await?,
        // A reset of everything else (or an explicit "*" entity) wipes all
        // synced tables.
        _ => reset_all(db).await?,
    }
    db.metadata()
        .set(open_course_db::metadata::KEY_RESET_AT, &reset_at)
        .await?;
    Ok(())
}
