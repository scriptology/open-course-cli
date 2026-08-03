//! First-device binding: detecting cloud/local curriculum conflicts and the
//! adopt / replace resolutions, including the progress-merge choice.
//!
//! Curriculum is a sync-once entity: the first machine to push marks the
//! canonical version on the server; any other machine enabling sync for the
//! same pair must adopt the cloud curriculum or replace it with its own.

use std::collections::HashSet;

use open_course_db::Database;
use open_course_db::outbox::{ENTITY_TOPIC, OP_UPSERT};

use crate::client::{SyncClient, check_status};
use crate::error::{PushError, SyncError};
use crate::protocol::{Change, CurriculumPayload, PullResponse};

/// What a device enabling sync for a pair faces.
#[derive(Debug)]
pub enum BindScenario {
    /// No curriculum in the cloud: simply push the local one.
    FreshLocal,
    /// The cloud has a curriculum and the local database is empty: simply
    /// pull.
    FreshCloud,
    /// Both sides have data; the user must choose adopt or replace. The
    /// payload carries the server's canonical curriculum.
    Conflict(CurriculumPayload),
}

/// What to do with local progress when adopting the cloud curriculum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMerge {
    /// Keep local progress and merge by last-writer-wins on `updated_at`
    /// (local rows with unknown timestamps lose to cloud rows; local rows
    /// the cloud lacks are kept and pushed later).
    Merge,
    /// Discard local progress and learning items, then pull everything
    /// from the cloud. The wipe is physical and bypasses the outbox, so no
    /// delete operations ever reach the server.
    StartFromCloud,
}

/// What `merge_bind` did, for UI reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeReport {
    /// Topics present on both sides (winner picked by `updated_at`).
    pub topics_merged: usize,
    /// Topics only the local device had (kept and pushed).
    pub topics_local_only: usize,
    /// Topics only the cloud had (pulled).
    pub topics_cloud_only: usize,
    /// The server revision after the merge push.
    pub revision: i64,
}

impl SyncClient {
    /// Fetches the pull feed without applying it — a probe for bind
    /// decisions.
    pub async fn preview_pull(&self, pair_id: &str, since: i64) -> Result<PullResponse, SyncError> {
        let resp = self
            .authorized(self.http_ref().get(self.url("/v1/sync/pull")))
            .query(&[("pairId", pair_id), ("since", since.to_string().as_str())])
            .send()
            .await?;
        let resp = check_status(resp).await?;
        Ok(resp.json().await?)
    }

    /// Determines the bind scenario for a pair: whether the cloud already
    /// has a curriculum and whether the local database holds any data.
    pub async fn first_bind_choices(
        &self,
        db: &Database,
        pair_id: &str,
    ) -> Result<BindScenario, SyncError> {
        let cloud = self.preview_pull(pair_id, 0).await?;
        let cloud_topics = topics_from_changes(&cloud.changes);
        let local_has_data = !db.curriculum().read_all().await?.topics.is_empty()
            || !db.history().read_all().await?.is_empty();

        Ok(if cloud_topics.is_empty() {
            BindScenario::FreshLocal
        } else if !local_has_data {
            BindScenario::FreshCloud
        } else {
            let version = cloud_topics.iter().map(|t| t.version).max().unwrap_or(1);
            BindScenario::Conflict(CurriculumPayload {
                revision: cloud.revision,
                version,
                topics: cloud_topics,
            })
        })
    }

    /// Adopt the server's canonical curriculum: the canonical topics are
    /// persisted verbatim (timestamps kept), local topics missing from the
    /// canon are tombstoned (their progress rows stay — they become
    /// inactive naturally once the topic is gone from reads), and a full
    /// pull follows to reconcile everything else by last-writer-wins.
    pub async fn adopt_cloud_curriculum(
        &self,
        db: &Database,
        pair_id: &str,
        payload: &CurriculumPayload,
        merge: ProgressMerge,
    ) -> Result<(), SyncError> {
        let canonical_ids: HashSet<&str> = payload.topics.iter().map(|t| t.id.as_str()).collect();
        let local = db.curriculum().read_all().await?;

        for topic in &payload.topics {
            db.curriculum().upsert_with_timestamps(topic).await?;
        }
        for topic in &local.topics {
            if !canonical_ids.contains(topic.id.as_str()) {
                db.curriculum().delete_by_topic_id(&topic.id).await?;
            }
        }

        if merge == ProgressMerge::StartFromCloud {
            // Physical wipe bypassing the outbox: nothing is pushed back.
            db.progress().reset().await?;
            db.learning_items().reset().await?;
        }

        db.metadata()
            .set_cloud_curriculum_version(payload.version)
            .await?;
        // Adopt + full pull: everything else (progress, sessions, items)
        // reconciles by last-writer-wins.
        db.metadata().set_last_pulled_seq(0).await?;
        self.pull(db, pair_id).await?;
        Ok(())
    }

    /// Replace the server's canonical curriculum with the local one: all
    /// local topics are stamped with `max(cloud, local) + 1` as the new
    /// canonical version and pushed with the `forceCurriculum` flag, which
    /// obliges the server to replace the canon.
    pub async fn replace_cloud_curriculum(
        &self,
        db: &Database,
        pair_id: &str,
    ) -> Result<i64, PushError> {
        let cloud_version = db
            .metadata()
            .cloud_curriculum_version()
            .await
            .map_err(SyncError::from)?
            .unwrap_or(0);
        let local = db.curriculum().read_all().await.map_err(SyncError::from)?;
        let new_version = cloud_version.max(local.version).max(0) + 1;
        let now = chrono::Utc::now().to_rfc3339();

        for topic in &local.topics {
            let mut stamped = topic.clone();
            stamped.version = new_version;
            stamped.updated_at = Some(now.clone());
            db.curriculum()
                .upsert_with_timestamps(&stamped)
                .await
                .map_err(SyncError::from)?;
            let payload = serde_json::to_string(&stamped).map_err(SyncError::from)?;
            db.outbox()
                .append(OP_UPSERT, ENTITY_TOPIC, &stamped.id, &payload)
                .await
                .map_err(SyncError::from)?;
        }

        let revision = self.push_inner(db, pair_id, true).await?;
        db.metadata()
            .set_cloud_curriculum_version(new_version)
            .await
            .map_err(SyncError::from)?;
        Ok(revision)
    }

    /// Conflict-free bind for the automatic sync-all flow: merges the local
    /// and cloud curricula instead of asking the user to adopt or replace.
    ///
    /// Per topic id the row with the later `updated_at` wins (ties and
    /// missing timestamps go to the cloud — it carries the canonical
    /// revision). Topics present on only one side are kept. Anything
    /// changed locally since the last sync has a newer `updated_at` by
    /// construction, so no unpushed local edit is lost. The merged set is
    /// stamped with `max(cloud, local) + 1` and pushed with
    /// `forceCurriculum`; a full pull then reconciles progress, sessions
    /// and learning items by last-writer-wins.
    pub async fn merge_bind(&self, db: &Database, pair_id: &str) -> Result<MergeReport, PushError> {
        let cloud = self.preview_pull(pair_id, 0).await?;
        let cloud_topics = topics_from_changes(&cloud.changes);
        let cloud_version = cloud_topics.iter().map(|t| t.version).max().unwrap_or(0);
        let local = db.curriculum().read_all().await.map_err(SyncError::from)?;

        let mut merged: Vec<open_course_core::curriculum::Topic> = Vec::new();
        let mut topics_merged = 0usize;
        let mut topics_local_only = 0usize;
        for cloud_topic in &cloud_topics {
            match local.topics.iter().find(|t| t.id == cloud_topic.id) {
                Some(local_topic) => {
                    topics_merged += 1;
                    // Cloud wins ties and missing local timestamps.
                    let local_wins = match (
                        local_topic.updated_at.as_deref(),
                        cloud_topic.updated_at.as_deref(),
                    ) {
                        (Some(l), Some(c)) => l > c,
                        _ => false,
                    };
                    merged.push(if local_wins {
                        local_topic.clone()
                    } else {
                        cloud_topic.clone()
                    });
                }
                None => {
                    merged.push(cloud_topic.clone());
                }
            }
        }
        let topics_cloud_only = merged.len() - topics_merged;
        for local_topic in &local.topics {
            if !cloud_topics.iter().any(|t| t.id == local_topic.id) {
                topics_local_only += 1;
                merged.push(local_topic.clone());
            }
        }

        let new_version = cloud_version.max(local.version).max(0) + 1;
        let now = chrono::Utc::now().to_rfc3339();
        for topic in &merged {
            let mut stamped = topic.clone();
            stamped.version = new_version;
            stamped.updated_at = Some(now.clone());
            db.curriculum()
                .upsert_with_timestamps(&stamped)
                .await
                .map_err(SyncError::from)?;
            let payload = serde_json::to_string(&stamped).map_err(SyncError::from)?;
            db.outbox()
                .append(OP_UPSERT, ENTITY_TOPIC, &stamped.id, &payload)
                .await
                .map_err(SyncError::from)?;
        }
        // Local topics that lost the merge (not possible by construction —
        // the merged set is a superset of both sides) need no tombstoning.

        let revision = self.push_inner(db, pair_id, true).await?;
        db.metadata()
            .set_cloud_curriculum_version(new_version)
            .await
            .map_err(SyncError::from)?;
        // Full pull: progress, sessions and learning items reconcile by
        // last-writer-wins; the echo of our own topics no-ops.
        db.metadata()
            .set_last_pulled_seq(0)
            .await
            .map_err(SyncError::from)?;
        self.pull(db, pair_id).await?;

        Ok(MergeReport {
            topics_merged,
            topics_local_only,
            topics_cloud_only,
            revision,
        })
    }
}

/// Reconstructs the current topic set from a full change feed.
fn topics_from_changes(changes: &[Change]) -> Vec<open_course_core::curriculum::Topic> {
    let mut topics: Vec<open_course_core::curriculum::Topic> = Vec::new();
    let mut sorted: Vec<&Change> = changes.iter().collect();
    sorted.sort_by_key(|c| c.seq);
    for change in sorted {
        if change.entity != "topic" {
            continue;
        }
        if crate::protocol::op_is_upsert(&change.op)
            && let Some(payload) = &change.payload
            && let Ok(topic) = serde_json::from_value(payload.clone())
        {
            let topic: open_course_core::curriculum::Topic = topic;
            match topics.iter_mut().find(|t| t.id == topic.id) {
                Some(existing) => *existing = topic,
                None => topics.push(topic),
            }
        } else if crate::protocol::op_is_delete(&change.op) {
            topics.retain(|t| t.id != change.entity_id);
        } else if crate::protocol::op_is_tombstone_reset(&change.op) {
            topics.clear();
        }
    }
    topics
}
