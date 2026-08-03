//! Sync orchestrator: the single entry point for every background sync
//! operation. All triggers (app start, finished session analysis, local
//! data changes, login) go through `schedule`, which coalesces overlapping
//! requests and picks the right operation. UI-facing report types and the
//! `SyncMessage` channel live in `ui::views::settings::account`; this module
//! only decides WHAT runs and spawns the tasks.

use std::sync::Arc;

use open_course_config::{pair_db_path, resolve_sync_server_url};
use open_course_db::Database;
use open_course_sync::{BindScenario, PushError, SyncClient, SyncError, TokenStore};

use crate::app::AppState;
use crate::ui::views::settings::account::{PairSyncStatus, SyncFailure, SyncMessage, SyncReport};

/// Why a background sync was scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncTrigger {
    /// Right after a successful login: bind and sync every pair (with the
    /// SyncAll progress view).
    AfterLogin,
    /// Application start: quiet pull of every pair with sync enabled.
    AppStart,
    /// A session's analysis was applied: push the active pair.
    AfterAnalysis,
    /// Local synced data changed outside a session (curriculum generation,
    /// data reset): push the active pair.
    DataChanged,
}

/// Coalescing state for the orchestrator: one run at a time, a repeated
/// trigger while running is remembered and re-run on completion.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncSchedulerState {
    pub active: bool,
    pub pending: Option<SyncTrigger>,
}

/// One-off operations shared by the account view (manual sync, bind
/// follow-ups). Moved here so every spawned sync task lives in one module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncKind {
    /// Explicit "Sync now": pull, then push (even when the per-pair toggle
    /// is off — the user asked explicitly).
    Manual,
    /// Push after a finished session (only when the toggle is on).
    AfterSession,
    /// Pull only (bind of an empty local database to a cloud pair).
    PullOnly,
}

/// The single entry point for background sync. Overlapping requests are
/// coalesced: the running task finishes, then the LAST pending trigger is
/// re-run once.
pub async fn schedule(state: &mut AppState, trigger: SyncTrigger) {
    if state.sync.active {
        state.sync.pending = Some(trigger);
        return;
    }
    match trigger {
        SyncTrigger::AfterAnalysis | SyncTrigger::DataChanged => {
            if !state.db.metadata().sync_enabled().await.unwrap_or(false) {
                return;
            }
            state.sync.active = true;
            spawn_sync(state, SyncKind::AfterSession);
        }
        SyncTrigger::AppStart => {
            state.sync.active = true;
            spawn_pull_all(state);
        }
        SyncTrigger::AfterLogin => {
            state.sync.active = true;
            spawn_sync_all(state);
        }
    }
}

/// Marks the running orchestrator task as finished and drains the pending
/// trigger, if any. Called from `apply_sync_message` on every terminating
/// sync message.
pub async fn finish(state: &mut AppState) {
    state.sync.active = false;
    let pending = state.sync.pending.take();
    if let Some(trigger) = pending {
        schedule(state, trigger).await;
    }
}

/// Spawns a one-off operation for the active pair (used by the account
/// view: manual sync and the FreshLocal/FreshCloud bind follow-ups).
pub(crate) fn spawn_sync(state: &AppState, kind: SyncKind) {
    let data_dir = state.data_dir.clone();
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let pair_id = state
        .config
        .as_ref()
        .map(|c| c.active_pair.clone())
        .unwrap_or_default();
    let db = state.db.as_ref().clone();
    let tx = state.sync_tx.clone();
    tokio::spawn(async move {
        let outcome = run_sync(&data_dir, &base_url, db, pair_id, kind).await;
        match outcome {
            Some(SyncOutcome::Done(report)) => {
                let _ = tx.send(SyncMessage::SyncFinished(report)).await;
            }
            Some(SyncOutcome::Conflict(payload)) => {
                let _ = tx.send(SyncMessage::CurriculumConflict(payload)).await;
            }
            // The orchestrator counts on a terminating message; a silent
            // skip (signed out, toggle off) still releases the scheduler.
            None => {
                let _ = tx.send(SyncMessage::SchedulerIdle).await;
            }
        }
    });
}

/// What a background sync task produced.
enum SyncOutcome {
    Done(Result<SyncReport, SyncFailure>),
    Conflict(open_course_sync::CurriculumPayload),
}

/// `None` means "skip silently" (signed out, or disabled for OnStart /
/// AfterSession).
async fn run_sync(
    data_dir: &std::path::Path,
    base_url: &str,
    db: Database,
    pair_id: String,
    kind: SyncKind,
) -> Option<SyncOutcome> {
    let store = TokenStore::new(data_dir.to_path_buf());
    let token = match store.load().await {
        Ok(Some(token)) => token,
        Ok(None) if kind == SyncKind::Manual => {
            return Some(SyncOutcome::Done(Err(SyncFailure::unauthorized())));
        }
        Ok(None) => return None,
        Err(e) => return Some(SyncOutcome::Done(Err(SyncFailure::other(e.to_string())))),
    };
    if kind != SyncKind::Manual && !db.metadata().sync_enabled().await.unwrap_or(false) {
        return None;
    }
    let client = match SyncClient::new(base_url) {
        Ok(client) => client.with_access_token(token.access_token),
        Err(e) => return Some(SyncOutcome::Done(Err(SyncFailure::other(e.to_string())))),
    };

    let result = match kind {
        SyncKind::PullOnly => client
            .pull(&db, &pair_id)
            .await
            .map(SyncReport::sync)
            .map_err(map_sync_err),
        SyncKind::AfterSession => match client.push(&db, &pair_id).await {
            Ok(revision) => Ok(SyncReport::sync(revision)),
            Err(PushError::CurriculumConflict(payload)) => {
                return Some(SyncOutcome::Conflict(payload));
            }
            Err(e) => Err(map_push_err(e)),
        },
        SyncKind::Manual => {
            if let Err(e) = client.pull(&db, &pair_id).await {
                Err(map_sync_err(e))
            } else {
                match client.push(&db, &pair_id).await {
                    Ok(revision) => Ok(SyncReport::sync(revision)),
                    Err(PushError::CurriculumConflict(payload)) => {
                        return Some(SyncOutcome::Conflict(payload));
                    }
                    Err(e) => Err(map_push_err(e)),
                }
            }
        }
    };
    if result.is_ok() {
        let _ = db
            .metadata()
            .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
            .await;
    }
    Some(SyncOutcome::Done(result))
}

pub(crate) fn map_push_err(e: PushError) -> SyncFailure {
    match e {
        // The conflict body is handled separately; the outbox and all local
        // data stay intact.
        PushError::CurriculumConflict(_) => SyncFailure::conflict(),
        PushError::Sync(SyncError::Unauthorized) => SyncFailure::unauthorized(),
        PushError::Sync(e) => SyncFailure::other(e.to_string()),
    }
}

pub(crate) fn map_sync_err(e: SyncError) -> SyncFailure {
    match e {
        SyncError::Unauthorized => SyncFailure::unauthorized(),
        other => SyncFailure::other(other.to_string()),
    }
}

/// App start: a quiet pull of every pair with sync enabled. Only a rejected
/// token is surfaced (offline-first); per-pair network failures are
/// swallowed. Always terminates with a message so the scheduler idles.
fn spawn_pull_all(state: &AppState) {
    let data_dir = state.data_dir.clone();
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let pair_ids = pair_ids(state);
    let active_pair = state
        .config
        .as_ref()
        .map(|c| c.active_pair.clone())
        .unwrap_or_default();
    let active_db = Arc::clone(&state.db);
    let tx = state.sync_tx.clone();
    tokio::spawn(async move {
        let token = match TokenStore::new(data_dir.clone()).load().await {
            Ok(Some(token)) => token,
            // Signed out (or an unreadable store): nothing to pull.
            _ => {
                let _ = tx.send(SyncMessage::SchedulerIdle).await;
                return;
            }
        };
        let client = match SyncClient::new(&base_url) {
            Ok(client) => client.with_access_token(token.access_token),
            Err(_) => {
                let _ = tx.send(SyncMessage::SchedulerIdle).await;
                return;
            }
        };
        let mut unauthorized = false;
        for pair_id in &pair_ids {
            let Some(db) = open_pair_db(&data_dir, &active_db, &active_pair, pair_id).await else {
                continue;
            };
            if !db.metadata().sync_enabled().await.unwrap_or(false) {
                continue;
            }
            match client.pull_with_timeout(&db, pair_id).await {
                Ok(_) => {
                    let _ = db
                        .metadata()
                        .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
                        .await;
                }
                Err(SyncError::Unauthorized) => {
                    unauthorized = true;
                    break;
                }
                // Offline-first: quiet.
                Err(_) => {}
            }
        }
        let msg = if unauthorized {
            SyncMessage::PullOnStartFinished(Err(SyncFailure::unauthorized()))
        } else {
            // The report's revision is unused by the handler; it only
            // triggers a status refresh.
            SyncMessage::PullOnStartFinished(Ok(SyncReport::sync(0)))
        };
        let _ = tx.send(msg).await;
    });
}

/// After login: bind and sync EVERY pair, reporting per-pair progress to
/// the SyncAll view. Conflicts are resolved by `merge_bind` (last-writer-
/// wins by `updated_at`), no dialogs.
fn spawn_sync_all(state: &AppState) {
    let data_dir = state.data_dir.clone();
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let pair_ids = pair_ids(state);
    let active_pair = state
        .config
        .as_ref()
        .map(|c| c.active_pair.clone())
        .unwrap_or_default();
    let active_db = Arc::clone(&state.db);
    let tx = state.sync_tx.clone();
    tokio::spawn(async move {
        let token = match TokenStore::new(data_dir.clone()).load().await {
            Ok(Some(token)) => token,
            Ok(None) => {
                for pair_id in &pair_ids {
                    let _ = tx
                        .send(SyncMessage::SyncAllProgress {
                            pair_id: pair_id.clone(),
                            status: PairSyncStatus::Unauthorized,
                        })
                        .await;
                }
                let _ = tx
                    .send(SyncMessage::SyncAllFinished {
                        failed: pair_ids.len(),
                    })
                    .await;
                return;
            }
            Err(e) => {
                let message = e.to_string();
                for pair_id in &pair_ids {
                    let _ = tx
                        .send(SyncMessage::SyncAllProgress {
                            pair_id: pair_id.clone(),
                            status: PairSyncStatus::Failed(message.clone()),
                        })
                        .await;
                }
                let _ = tx
                    .send(SyncMessage::SyncAllFinished {
                        failed: pair_ids.len(),
                    })
                    .await;
                return;
            }
        };
        let client = match SyncClient::new(&base_url) {
            Ok(client) => client.with_access_token(token.access_token),
            Err(e) => {
                let message = e.to_string();
                for pair_id in &pair_ids {
                    let _ = tx
                        .send(SyncMessage::SyncAllProgress {
                            pair_id: pair_id.clone(),
                            status: PairSyncStatus::Failed(message.clone()),
                        })
                        .await;
                }
                let _ = tx
                    .send(SyncMessage::SyncAllFinished {
                        failed: pair_ids.len(),
                    })
                    .await;
                return;
            }
        };

        let mut failed = 0usize;
        for pair_id in &pair_ids {
            let _ = tx
                .send(SyncMessage::SyncAllProgress {
                    pair_id: pair_id.clone(),
                    status: PairSyncStatus::Running,
                })
                .await;
            let status = match open_pair_db(&data_dir, &active_db, &active_pair, pair_id).await {
                Some(db) => bind_and_sync(&client, &db, pair_id).await,
                None => PairSyncStatus::Failed("database unavailable".to_string()),
            };
            if matches!(
                status,
                PairSyncStatus::Failed(_) | PairSyncStatus::Unauthorized
            ) {
                failed += 1;
            }
            let _ = tx
                .send(SyncMessage::SyncAllProgress {
                    pair_id: pair_id.clone(),
                    status,
                })
                .await;
        }
        let _ = tx.send(SyncMessage::SyncAllFinished { failed }).await;
    });
}

/// Binds one pair and runs the first sync: push for a cloud-empty pair,
/// pull for a locally-empty one, `merge_bind` for a conflict. Enables sync
/// for the pair on success.
async fn bind_and_sync(client: &SyncClient, db: &Database, pair_id: &str) -> PairSyncStatus {
    let scenario = match client.first_bind_choices(db, pair_id).await {
        Ok(scenario) => scenario,
        Err(SyncError::Unauthorized) => return PairSyncStatus::Unauthorized,
        Err(e) => return PairSyncStatus::Failed(e.to_string()),
    };
    let result = match scenario {
        BindScenario::FreshLocal => client
            .push(db, pair_id)
            .await
            .map(|_| PairSyncStatus::Done)
            .map_err(push_err_status),
        BindScenario::FreshCloud => client
            .pull(db, pair_id)
            .await
            .map(|_| PairSyncStatus::Done)
            .map_err(sync_err_status),
        BindScenario::Conflict(_) => client
            .merge_bind(db, pair_id)
            .await
            .map(PairSyncStatus::Merged)
            .map_err(push_err_status),
    };
    match result {
        Ok(status) => {
            let _ = db.metadata().set_sync_enabled(true).await;
            let _ = db
                .metadata()
                .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
                .await;
            status
        }
        Err(status) => status,
    }
}

fn push_err_status(e: PushError) -> PairSyncStatus {
    match e {
        PushError::Sync(SyncError::Unauthorized) => PairSyncStatus::Unauthorized,
        other => PairSyncStatus::Failed(other.to_string()),
    }
}

fn sync_err_status(e: SyncError) -> PairSyncStatus {
    match e {
        SyncError::Unauthorized => PairSyncStatus::Unauthorized,
        other => PairSyncStatus::Failed(other.to_string()),
    }
}

/// Pair ids from the config, in config order.
fn pair_ids(state: &AppState) -> Vec<String> {
    state
        .config
        .as_ref()
        .map(|c| c.pairs.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default()
}

/// Opens the pair's database; the active pair reuses the app's connection.
async fn open_pair_db(
    data_dir: &std::path::Path,
    active_db: &Arc<Database>,
    active_pair: &str,
    pair_id: &str,
) -> Option<Database> {
    if pair_id == active_pair {
        return Some(active_db.as_ref().clone());
    }
    Database::connect(&pair_db_path(data_dir, pair_id))
        .await
        .ok()
}
