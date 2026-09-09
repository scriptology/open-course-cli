//! Sync orchestrator: the single entry point for every background sync
//! operation. All triggers (app start, finished session analysis, local
//! data changes, login) go through `schedule`, which coalesces overlapping
//! requests and picks the right operation. UI-facing report types and the
//! `SyncMessage` channel live in `ui::views::settings::account`; this module
//! only decides WHAT runs and spawns the tasks.

use std::sync::Arc;
use std::time::Duration;

use open_course_config::{merge_remote_pairs, pair_db_path, resolve_sync_server_url, write_config};
use open_course_db::Database;
use open_course_sync::{
    BindScenario, PushError, SyncClient, SyncError, TokenStore, backfill_outbox,
};

use crate::app::{AppState, View};
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
    /// Explicit "Sync now": bind and sync every pair (with the SyncAll
    /// progress view), even pairs whose per-pair toggle is off — the user
    /// asked explicitly.
    Manual,
}

/// Coalescing state for the orchestrator: one run at a time, a repeated
/// trigger while running is remembered and re-run on completion.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncSchedulerState {
    pub active: bool,
    pub pending: Option<SyncTrigger>,
}

/// Per-pair ceiling for the quiet app-start pull: the short HTTP timeout
/// plus room for opening the pair's database. A hung step is skipped — it
/// must never wedge the scheduler.
const PAIR_PULL_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-pair ceiling for a sync-all run (a bind can backfill and push a large
/// outbox: several retried requests). A hung pair is marked failed.
const PAIR_SYNC_TIMEOUT: Duration = Duration::from_secs(120);

/// Appends a timestamped line to `.open-course-cli/sync-debug.log`
/// (diagnostics for sync scheduling/hang investigations).
fn debug_log(data_dir: &std::path::Path, message: &str) {
    use std::io::Write;
    let path = open_course_config::open_course_dir(data_dir).join("sync-debug.log");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{} {}", chrono::Utc::now().to_rfc3339(), message);
}

/// One-off operations shared by the account view (the FreshLocal/FreshCloud
/// bind follow-ups). Moved here so every spawned sync task lives in one
/// module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncKind {
    /// Push after a finished session (only when the toggle is on).
    AfterSession,
    /// Pull only (bind of an empty local database to a cloud pair).
    PullOnly,
}

/// The single entry point for background sync. Overlapping requests are
/// coalesced: the running task finishes, then the LAST pending trigger is
/// re-run once.
pub async fn schedule(state: &mut AppState, trigger: SyncTrigger) {
    debug_log(&state.data_dir, &format!("schedule: {trigger:?}"));
    if state.sync.active {
        debug_log(
            &state.data_dir,
            &format!("schedule: {trigger:?} coalesced (another run is active)"),
        );
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
            discover_pairs(state).await;
            state.sync.active = true;
            spawn_sync_all(state, SyncAllMode::AfterLogin);
        }
        SyncTrigger::Manual => {
            discover_pairs(state).await;
            state.sync.active = true;
            spawn_sync_all(state, SyncAllMode::Manual);
        }
    }
}

/// Fetches the account's pair list from the server and merges pairs the
/// config does not know yet (created on the web or another device) into it,
/// so the sync-all run that follows binds and pulls them. Best-effort: any
/// failure (signed out, offline) keeps the local pair list.
async fn discover_pairs(state: &mut AppState) {
    debug_log(&state.data_dir, "discover: fetching remote pairs");
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let token = match TokenStore::new(state.data_dir.clone()).load().await {
        Ok(Some(token)) => token,
        _ => {
            debug_log(&state.data_dir, "discover: no token, skipped");
            return;
        }
    };
    let client = match SyncClient::new(&base_url) {
        Ok(client) => client.with_access_token(token.access_token),
        Err(_) => return,
    };
    let remote = match client.list_pairs().await {
        Ok(pairs) => pairs,
        Err(e) => {
            debug_log(&state.data_dir, &format!("discover: list failed: {e}"));
            return;
        }
    };
    let Some(config) = state.config.as_mut() else {
        return;
    };
    let added = merge_remote_pairs(config, &remote);
    debug_log(
        &state.data_dir,
        &format!(
            "discover: {} remote pair(s), {} added",
            remote.len(),
            added.len()
        ),
    );
    if added.is_empty() {
        return;
    }
    if write_config(config, &state.data_dir).is_err() {
        return;
    }
    // The SyncAll view seeded its rows before the orchestrator ran; add rows
    // for the discovered pairs so their progress is visible too.
    if state.view == View::SyncAll && !state.sync_all.done {
        for pair in &config.pairs {
            if added.contains(&pair.id) {
                state
                    .sync_all
                    .rows
                    .push(crate::ui::views::sync_all::PairSyncRow {
                        pair_id: pair.id.clone(),
                        title: format!(
                            "{} → {}",
                            pair.profile.native_language, pair.profile.target_language
                        ),
                        status: None,
                    });
            }
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
/// view: the FreshLocal/FreshCloud bind follow-ups).
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

/// `None` means "skip silently" (signed out, or the toggle is off).
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
        Ok(None) => return None,
        Err(e) => return Some(SyncOutcome::Done(Err(SyncFailure::other(e.to_string())))),
    };
    if !db.metadata().sync_enabled().await.unwrap_or(false) {
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
/// swallowed. A supervisor sends the terminating message even when the pull
/// task panics, so the scheduler never wedges.
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
    let inner = tokio::spawn(async move {
        debug_log(&data_dir, "pull-all: started");
        let token = match TokenStore::new(data_dir.clone()).load().await {
            Ok(Some(token)) => token,
            // Signed out (or an unreadable store): nothing to pull.
            _ => return None,
        };
        let client = match SyncClient::new(&base_url) {
            Ok(client) => client.with_access_token(token.access_token),
            Err(_) => return None,
        };
        let mut unauthorized = false;
        for pair_id in &pair_ids {
            let step = async {
                let db = open_pair_db(&data_dir, &active_db, &active_pair, pair_id).await?;
                if !db.metadata().sync_enabled().await.unwrap_or(false) {
                    return Some((db, false));
                }
                Some((db, true))
            };
            let Some((db, enabled)) = tokio::time::timeout(PAIR_PULL_TIMEOUT, step)
                .await
                .unwrap_or_else(|_| {
                    debug_log(
                        &data_dir,
                        &format!("pull-all: {pair_id}: open/check TIMED OUT"),
                    );
                    None
                })
            else {
                continue;
            };
            if !enabled {
                continue;
            }
            debug_log(&data_dir, &format!("pull-all: {pair_id}: pulling"));
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
        debug_log(&data_dir, "pull-all: finished");
        Some(unauthorized)
    });
    tokio::spawn(async move {
        let msg = match inner.await {
            // The report's revision is unused by the handler; it only
            // triggers a status refresh.
            Ok(Some(false)) => SyncMessage::PullOnStartFinished(Ok(SyncReport::sync(0))),
            Ok(Some(true)) => SyncMessage::PullOnStartFinished(Err(SyncFailure::unauthorized())),
            // Signed out, no client, or the pull task panicked: release the
            // scheduler quietly.
            Ok(None) | Err(_) => SyncMessage::SchedulerIdle,
        };
        let _ = tx.send(msg).await;
    });
}

/// Which per-pair operation a sync-all run performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncAllMode {
    /// Right after login: every pair goes through the first bind.
    AfterLogin,
    /// Explicit "Sync now": bound pairs pull and push; never-bound pairs
    /// (created after the login) go through the first bind.
    Manual,
}

/// Binds and syncs EVERY pair, reporting per-pair progress to the SyncAll
/// view. Conflicts are resolved by `merge_bind` (last-writer-wins by
/// `updated_at`), no dialogs.
///
/// The run lives in an inner task whose abort handle the SyncAll view uses
/// for Esc-cancellation; a supervisor awaits it and ALWAYS sends the
/// terminating `SyncAllFinished` — a panic in the sync code must never leave
/// the view spinning forever (and the scheduler wedged) again.
fn spawn_sync_all(state: &mut AppState, mode: SyncAllMode) {
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
    let total = pair_ids.len();
    debug_log(
        &data_dir,
        &format!("sync-all: started ({total} pair(s), mode {mode:?})"),
    );
    // (index of the pair currently syncing, its id) — for panic reporting.
    let current = Arc::new(std::sync::Mutex::new((0usize, String::new())));
    let inner = {
        let current = Arc::clone(&current);
        let tx = tx.clone();
        let data_dir = data_dir.clone();
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
                    return pair_ids.len();
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
                    return pair_ids.len();
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
                    return pair_ids.len();
                }
            };

            let mut failed = 0usize;
            for (index, pair_id) in pair_ids.iter().enumerate() {
                *current.lock().unwrap() = (index, pair_id.clone());
                let _ = tx
                    .send(SyncMessage::SyncAllProgress {
                        pair_id: pair_id.clone(),
                        status: PairSyncStatus::Running,
                    })
                    .await;
                debug_log(&data_dir, &format!("sync-all: {pair_id}: running"));
                let step = async {
                    match open_pair_db(&data_dir, &active_db, &active_pair, pair_id).await {
                        Some(db) => match mode {
                            SyncAllMode::AfterLogin => {
                                bind_and_sync(&data_dir, &client, &db, pair_id).await
                            }
                            SyncAllMode::Manual => {
                                manual_sync_pair(&data_dir, &client, &db, pair_id).await
                            }
                        },
                        None => PairSyncStatus::Failed("database unavailable".to_string()),
                    }
                };
                let status = match tokio::time::timeout(PAIR_SYNC_TIMEOUT, step).await {
                    Ok(status) => status,
                    Err(_) => {
                        debug_log(&data_dir, &format!("sync-all: {pair_id}: TIMED OUT"));
                        PairSyncStatus::Failed("timed out".to_string())
                    }
                };
                debug_log(&data_dir, &format!("sync-all: {pair_id}: {status:?}"));
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
            failed
        })
    };
    state.sync_all.abort = Some(inner.abort_handle());
    tokio::spawn(async move {
        let (failed, cancelled) = match inner.await {
            Ok(failed) => (failed, false),
            Err(e) if e.is_cancelled() => (0, true),
            Err(e) => {
                // The sync task panicked: report the pair it was on and
                // count every unfinished pair as failed.
                let (index, pair_id) = current.lock().unwrap().clone();
                let message = panic_message(&e);
                debug_log(
                    &data_dir,
                    &format!("sync-all: task panicked at {pair_id}: {message}"),
                );
                if !pair_id.is_empty() {
                    let _ = tx
                        .send(SyncMessage::SyncAllProgress {
                            pair_id,
                            status: PairSyncStatus::Failed(format!("internal error: {message}")),
                        })
                        .await;
                }
                (total.saturating_sub(index), false)
            }
        };
        debug_log(
            &data_dir,
            &format!("sync-all: finished (failed={failed}, cancelled={cancelled})"),
        );
        let _ = tx
            .send(SyncMessage::SyncAllFinished { failed, cancelled })
            .await;
    });
}

/// Extracts the panic message from a join error for the failure row.
fn panic_message(e: &tokio::task::JoinError) -> String {
    let panic = e.to_string();
    panic
        .strip_prefix("task panicked")
        .map(str::trim)
        .unwrap_or(&panic)
        .to_string()
}

/// Manual "Sync now" for one pair. A never-bound pair (created after the
/// login, so no bind ever ran for it) goes through `bind_and_sync`: this is
/// what uploads pairs that never reached the cloud. A bound pair pulls,
/// then pushes; a 409 curriculum conflict is auto-merged by `merge_bind`
/// (last-writer-wins), like the after-login run. Runs even when the
/// per-pair toggle is off — the user asked explicitly.
async fn manual_sync_pair(
    data_dir: &std::path::Path,
    client: &SyncClient,
    db: &Database,
    pair_id: &str,
) -> PairSyncStatus {
    // "Bound" marker: a canonical curriculum version is stamped by every
    // bind that pushed or merged topics; a pair bound by a pure pull (an
    // empty local database) has `last_pulled_seq` advanced instead.
    let bound = db
        .metadata()
        .cloud_curriculum_version()
        .await
        .unwrap_or(None)
        .is_some()
        || db.metadata().last_pulled_seq().await.unwrap_or(0) > 0;
    if !bound {
        return bind_and_sync(data_dir, client, db, pair_id).await;
    }
    if let Err(e) = client.pull(db, pair_id).await {
        return sync_err_status(e);
    }
    let status = match client.push(db, pair_id).await {
        Ok(_) => PairSyncStatus::Done,
        Err(PushError::CurriculumConflict(_)) => match client.merge_bind(db, pair_id).await {
            Ok(report) => PairSyncStatus::Merged(report),
            Err(e) => push_err_status(e),
        },
        Err(e) => push_err_status(e),
    };
    if matches!(status, PairSyncStatus::Done | PairSyncStatus::Merged(_)) {
        let _ = db
            .metadata()
            .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
            .await;
    }
    status
}

/// Binds one pair and runs the first sync: push for a cloud-empty pair,
/// pull for a locally-empty one, `merge_bind` for a conflict. Enables sync
/// for the pair on success.
async fn bind_and_sync(
    data_dir: &std::path::Path,
    client: &SyncClient,
    db: &Database,
    pair_id: &str,
) -> PairSyncStatus {
    let scenario = match client.first_bind_choices(db, pair_id).await {
        Ok(scenario) => scenario,
        Err(SyncError::Unauthorized) => return PairSyncStatus::Unauthorized,
        Err(e) => return PairSyncStatus::Failed(e.to_string()),
    };
    debug_log(
        data_dir,
        &format!(
            "sync-all: {pair_id}: bind scenario {}",
            match &scenario {
                BindScenario::FreshLocal => "FreshLocal",
                BindScenario::FreshCloud => "FreshCloud",
                BindScenario::Conflict(_) => "Conflict",
            }
        ),
    );
    let result = match scenario {
        BindScenario::FreshLocal => {
            // Pre-sync data never entered the outbox: enqueue it first,
            // otherwise the "fresh local" push is a no-op.
            let version = db.curriculum().read_all().await.ok().map(|c| c.version);
            match backfill_outbox(db, true).await.map_err(sync_err_status) {
                Err(status) => Err(status),
                Ok(()) => match client.push(db, pair_id).await {
                    Ok(_) => {
                        if let Some(version) = version {
                            let _ = db.metadata().set_cloud_curriculum_version(version).await;
                        }
                        Ok(PairSyncStatus::Done)
                    }
                    // The pair may already exist in the cloud with an
                    // empty canon (created on the web): the first topic
                    // push is a 409 by design — merge instead of failing.
                    // `merge_bind` stamps the cloud version itself.
                    Err(PushError::CurriculumConflict(_)) => client
                        .merge_bind(db, pair_id)
                        .await
                        .map(PairSyncStatus::Merged)
                        .map_err(push_err_status),
                    Err(e) => Err(push_err_status(e)),
                },
            }
        }
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
