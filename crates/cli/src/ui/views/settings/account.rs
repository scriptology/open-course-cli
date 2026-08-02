//! Account section: sign-in via the OAuth device flow, per-pair sync toggle,
//! manual sync, and sync status. All background work reports back through
//! the `SyncMessage` channel; this module is the UI's only sync client.

use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

use open_course_config::{resolve_sync_server_url, write_config};
use open_course_core::error::Result;
use open_course_db::Database;
use open_course_sync::{
    BindScenario, CurriculumPayload, DeviceCodeResponse, MeResponse, PollResult, ProgressMerge,
    PushError, SyncClient, SyncError, TokenBackend, TokenSet, TokenStore,
};

use crate::app::AppState;
use crate::ui::labels::SettingsLabels;
use crate::ui::widgets::{Toast, error_lines};

/// Messages from background sync tasks to the event loop.
#[derive(Debug)]
pub enum SyncMessage {
    /// Token presence + storage backend, probed when the section opens.
    AccountRefreshed {
        has_token: bool,
        backend: TokenBackend,
    },
    DeviceFlowStarted(std::result::Result<DeviceCodeResponse, String>),
    DeviceFlowExpired,
    LoginFinished(std::result::Result<LoginInfo, String>),
    /// Result of the bind-scenario probe when sync is being enabled.
    BindScenarioLoaded(std::result::Result<BindScenario, String>),
    /// Manual sync or push-after-session completed.
    SyncFinished(std::result::Result<SyncReport, SyncFailure>),
    /// Background pull-on-start completed.
    PullOnStartFinished(std::result::Result<SyncReport, SyncFailure>),
    /// A push was rejected with 409: the cloud's canonical curriculum
    /// differs. Carries the payload for the resolution dialog.
    CurriculumConflict(CurriculumPayload),
    /// Best-effort `/v1/me` result (`None` when unavailable).
    MeFetched(Option<MeResponse>),
}

#[derive(Debug)]
pub struct LoginInfo {
    pub email: Option<String>,
    pub device_id: String,
    pub subscription: Option<String>,
    pub backend: TokenBackend,
}

/// What a finished sync operation did, for the status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportAction {
    Sync,
    Adopt,
    Replace,
}

#[derive(Debug)]
pub struct SyncReport {
    pub revision: i64,
    pub action: ReportAction,
    pub topics: Option<usize>,
}

impl SyncReport {
    fn sync(revision: i64) -> Self {
        Self {
            revision,
            action: ReportAction::Sync,
            topics: None,
        }
    }
}

#[derive(Debug)]
pub struct SyncFailure {
    pub message: String,
    pub unauthorized: bool,
    pub conflict: bool,
}

impl SyncFailure {
    fn unauthorized() -> Self {
        Self {
            message: String::new(),
            unauthorized: true,
            conflict: false,
        }
    }

    fn conflict() -> Self {
        Self {
            message: String::new(),
            unauthorized: false,
            conflict: true,
        }
    }

    fn other(message: String) -> Self {
        Self {
            message,
            unauthorized: false,
            conflict: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginStatus {
    #[default]
    LoggedOut,
    Starting,
    WaitingConfirmation,
    Expired,
    LoggingIn,
    LoggedIn,
}

/// Which choice the bind dialog is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindDialogStep {
    /// Adopt the cloud curriculum, replace it with the local one, or cancel.
    Curriculum,
    /// Merge local progress with the cloud's or start from the cloud.
    Progress,
}

/// Modal first-bind / 409 conflict resolution dialog.
#[derive(Debug, Clone)]
pub struct BindDialog {
    pub step: BindDialogStep,
    /// The server's canonical curriculum (adopt target / conflict source).
    pub payload: CurriculumPayload,
    /// Whether the user has local progress worth asking about.
    pub has_local_progress: bool,
    /// Selected option row: 0 = adopt/merge, 1 = replace/start-from-cloud,
    /// 2 = cancel.
    pub selected: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AccountState {
    pub status: LoginStatus,
    pub user_code: Option<String>,
    pub verification_url: Option<String>,
    pub email: Option<String>,
    pub device_id: Option<String>,
    pub token_backend: Option<TokenBackend>,
    pub sync_enabled: bool,
    pub last_sync_at: Option<String>,
    pub outbox_len: Option<usize>,
    pub subscription: Option<String>,
    pub relogin_required: bool,
    /// Transient status line (sync result, info).
    pub notice: Option<String>,
    /// Error line, shown as-is in the shared error style.
    pub error: Option<String>,
    pub syncing: bool,
    /// Active action row.
    pub field: usize,
    /// Modal conflict-resolution dialog, if open.
    pub bind_dialog: Option<BindDialog>,
}

/// Number of action rows for the current login status.
pub fn action_count(account: &AccountState) -> usize {
    match account.status {
        LoginStatus::LoggedOut | LoginStatus::Expired => 1,
        LoginStatus::LoggedIn => 3,
        _ => 0,
    }
}

/// Loads the section state when it opens and probes the token store and
/// (best-effort) the subscription in the background.
pub async fn on_enter(state: &mut AppState) {
    let account = &mut state.settings.account;
    account.field = 0;
    account.notice = None;
    account.error = None;
    if let Some(config) = state.config.as_ref() {
        account.email = config.sync.as_ref().and_then(|s| s.account_email.clone());
        account.device_id = config.sync.as_ref().and_then(|s| s.device_id.clone());
    }
    let metadata = state.db.metadata();
    account.sync_enabled = metadata.sync_enabled().await.unwrap_or(false);
    account.last_sync_at = metadata.last_sync_at().await.ok().flatten();
    account.outbox_len = state.db.outbox().len().await.ok();

    let data_dir = state.data_dir.clone();
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let tx = state.sync_tx.clone();
    tokio::spawn(async move {
        let store = TokenStore::new(data_dir);
        let backend = store.backend();
        let token = store.load().await.ok().flatten();
        let has_token = token.is_some();
        let _ = tx
            .send(SyncMessage::AccountRefreshed { has_token, backend })
            .await;
        if let Some(token) = token
            && let Ok(client) = SyncClient::new(base_url)
        {
            let me = client.with_access_token(token.access_token).me().await.ok();
            let _ = tx.send(SyncMessage::MeFetched(me)).await;
        }
    });
}

/// Enter on the active action row.
pub async fn activate(state: &mut AppState) -> Result<()> {
    if state.settings.account.syncing {
        return Ok(());
    }
    match state.settings.account.status {
        LoginStatus::LoggedOut | LoginStatus::Expired => start_sign_in(state),
        LoginStatus::LoggedIn => match state.settings.account.field {
            0 => start_manual_sync(state),
            1 => toggle_sync(state).await?,
            2 => sign_out(state).await?,
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn start_sign_in(state: &mut AppState) {
    state.settings.account.status = LoginStatus::Starting;
    state.settings.account.notice = None;
    state.settings.account.error = None;
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let tx = state.sync_tx.clone();
    tokio::spawn(async move {
        let result = async {
            let client = SyncClient::new(base_url).map_err(|e| e.to_string())?;
            client.start_device_flow().await.map_err(|e| e.to_string())
        }
        .await;
        let _ = tx.send(SyncMessage::DeviceFlowStarted(result)).await;
    });
}

fn spawn_device_poll(state: &AppState, device: &DeviceCodeResponse) {
    let base_url = resolve_sync_server_url(state.config.as_ref());
    let data_dir = state.data_dir.clone();
    let tx = state.sync_tx.clone();
    let device_code = device.device_code.clone();
    let interval = Duration::from_secs(device.interval.max(1));
    let expires_in = device.expires_in;
    tokio::spawn(async move {
        let client = match SyncClient::new(&base_url) {
            Ok(client) => client,
            Err(e) => {
                let _ = tx
                    .send(SyncMessage::LoginFinished(Err(e.to_string())))
                    .await;
                return;
            }
        };
        let mut waited = 0u64;
        loop {
            tokio::time::sleep(interval).await;
            waited += interval.as_secs();
            match client.poll_device_flow(&device_code).await {
                Ok(PollResult::Pending) => {
                    if waited >= expires_in {
                        let _ = tx.send(SyncMessage::DeviceFlowExpired).await;
                        break;
                    }
                }
                Ok(PollResult::Expired) => {
                    let _ = tx.send(SyncMessage::DeviceFlowExpired).await;
                    break;
                }
                Ok(PollResult::Authorized(tokens)) => {
                    let _ = tx
                        .send(SyncMessage::LoginFinished(
                            finish_login(&data_dir, &base_url, tokens).await,
                        ))
                        .await;
                    break;
                }
                Err(e) => {
                    let _ = tx
                        .send(SyncMessage::LoginFinished(Err(e.to_string())))
                        .await;
                    break;
                }
            }
        }
    });
}

/// Saves the fresh tokens and fetches the account profile (best-effort).
async fn finish_login(
    data_dir: &std::path::Path,
    base_url: &str,
    tokens: TokenSet,
) -> std::result::Result<LoginInfo, String> {
    let store = TokenStore::new(data_dir.to_path_buf());
    let backend = store.backend();
    store.save(&tokens).await.map_err(|e| e.to_string())?;
    let client = SyncClient::new(base_url)
        .map_err(|e| e.to_string())?
        .with_access_token(tokens.access_token.clone());
    let me = client.me().await.ok();
    let email = me
        .as_ref()
        .map(|m| m.email.clone())
        .or(tokens.user_email.clone());
    Ok(LoginInfo {
        email,
        device_id: tokens.device_id.clone(),
        subscription: me.map(|m| m.subscription_status),
        backend,
    })
}

fn start_manual_sync(state: &mut AppState) {
    state.settings.account.syncing = true;
    state.settings.account.notice = None;
    state.settings.account.error = None;
    spawn_sync(state, SyncKind::Manual);
}

async fn toggle_sync(state: &mut AppState) -> Result<()> {
    if state.settings.account.sync_enabled {
        state.db.metadata().set_sync_enabled(false).await?;
        state.settings.account.sync_enabled = false;
        return Ok(());
    }
    // Enabling: probe the cloud first — the pair may already have a
    // canonical curriculum pushed by another device.
    let lang = crate::ui::labels::native_language_code(state.config.as_ref());
    state.settings.account.error = None;
    state.settings.account.notice = Some(
        crate::ui::labels::get_settings_labels(lang)
            .account_bind_checking
            .to_string(),
    );
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
        let result = async {
            let store = TokenStore::new(data_dir);
            let token = store
                .load()
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "unauthorized".to_string())?;
            let client = SyncClient::new(base_url)
                .map_err(|e| e.to_string())?
                .with_access_token(token.access_token);
            client
                .first_bind_choices(&db, &pair_id)
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        let _ = tx.send(SyncMessage::BindScenarioLoaded(result)).await;
    });
    Ok(())
}

async fn sign_out(state: &mut AppState) -> Result<()> {
    let store = TokenStore::new(state.data_dir.clone());
    let _ = store.delete().await;
    if let Some(config) = state.config.as_mut() {
        if let Some(sync) = config.sync.as_mut() {
            sync.account_email = None;
            sync.device_id = None;
        }
        write_config(config, &state.data_dir)?;
    }
    let account = &mut state.settings.account;
    account.status = LoginStatus::LoggedOut;
    account.email = None;
    account.device_id = None;
    account.subscription = None;
    account.token_backend = None;
    account.relogin_required = false;
    account.field = 0;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncKind {
    /// Explicit "Sync now": pull, then push (even when the per-pair toggle
    /// is off — the user asked explicitly).
    Manual,
    /// Push after a finished session (only when the toggle is on).
    AfterSession,
    /// Pull on application start (only when the toggle is on, short
    /// timeout, quiet failures).
    OnStart,
    /// Pull only (bind of an empty local database to a cloud pair).
    PullOnly,
}

/// Background pull on application start: never blocks, silently skips when
/// signed out or sync is disabled for the pair.
pub fn spawn_pull_on_start(state: &AppState) {
    spawn_sync(state, SyncKind::OnStart);
}

/// Background push after a session's analysis was applied.
pub async fn spawn_push_after_session(state: &AppState) {
    if state.db.metadata().sync_enabled().await.unwrap_or(false) {
        spawn_sync(state, SyncKind::AfterSession);
    }
}

/// What a background sync task produced.
enum SyncOutcome {
    Done(std::result::Result<SyncReport, SyncFailure>),
    Conflict(CurriculumPayload),
}

fn spawn_sync(state: &AppState, kind: SyncKind) {
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
        let Some(outcome) = run_sync(&data_dir, &base_url, db, pair_id, kind).await else {
            return;
        };
        match outcome {
            SyncOutcome::Done(report) => {
                let msg = match kind {
                    SyncKind::OnStart => SyncMessage::PullOnStartFinished(report),
                    _ => SyncMessage::SyncFinished(report),
                };
                let _ = tx.send(msg).await;
            }
            SyncOutcome::Conflict(payload) => {
                let _ = tx.send(SyncMessage::CurriculumConflict(payload)).await;
            }
        }
    });
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
        SyncKind::OnStart => client
            .pull_with_timeout(&db, &pair_id)
            .await
            .map(SyncReport::sync)
            .map_err(map_sync_err),
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

fn map_push_err(e: PushError) -> SyncFailure {
    match e {
        // The conflict body is handled in a future version; the outbox and
        // all local data stay intact.
        PushError::CurriculumConflict(_) => SyncFailure::conflict(),
        PushError::Sync(SyncError::Unauthorized) => SyncFailure::unauthorized(),
        PushError::Sync(e) => SyncFailure::other(e.to_string()),
    }
}

fn map_sync_err(e: SyncError) -> SyncFailure {
    match e {
        SyncError::Unauthorized => SyncFailure::unauthorized(),
        other => SyncFailure::other(other.to_string()),
    }
}

/// Applies a background sync message to the UI state. Called from the event
/// loop for every `SyncMessage`.
pub async fn apply_sync_message(state: &mut AppState, message: SyncMessage) {
    let lang = crate::ui::labels::native_language_code(state.config.as_ref());
    let labels = crate::ui::labels::get_settings_labels(lang);
    match message {
        SyncMessage::AccountRefreshed { has_token, backend } => {
            let account = &mut state.settings.account;
            account.token_backend = Some(backend);
            match account.status {
                LoginStatus::LoggedOut if has_token => account.status = LoginStatus::LoggedIn,
                LoginStatus::LoggedIn if !has_token => account.status = LoginStatus::LoggedOut,
                _ => {}
            }
        }
        SyncMessage::DeviceFlowStarted(Ok(device)) => {
            state.settings.account.user_code = Some(device.user_code.clone());
            state.settings.account.verification_url = Some(device.verification_url.clone());
            state.settings.account.status = LoginStatus::WaitingConfirmation;
            open_browser(&device.verification_url);
            spawn_device_poll(state, &device);
        }
        SyncMessage::DeviceFlowStarted(Err(e)) => {
            state.settings.account.status = LoginStatus::LoggedOut;
            state.settings.account.error = Some(e);
        }
        SyncMessage::DeviceFlowExpired => {
            if state.settings.account.status == LoginStatus::WaitingConfirmation {
                state.settings.account.status = LoginStatus::Expired;
            }
        }
        SyncMessage::LoginFinished(Ok(info)) => {
            let server_url = resolve_sync_server_url(state.config.as_ref());
            if let Some(config) = state.config.as_mut() {
                let sync = config.sync.get_or_insert_with(Default::default);
                sync.account_email = info.email.clone();
                sync.device_id = Some(info.device_id.clone());
                if sync.server_url.is_none() {
                    sync.server_url = Some(server_url);
                }
                if let Err(e) = write_config(config, &state.data_dir) {
                    state.settings.account.error = Some(e.to_string());
                }
            }
            let account = &mut state.settings.account;
            account.status = LoginStatus::LoggedIn;
            account.email = info.email;
            account.device_id = Some(info.device_id);
            account.subscription = info.subscription;
            account.token_backend = Some(info.backend);
            account.relogin_required = false;
            account.field = 0;
        }
        SyncMessage::LoginFinished(Err(e)) => {
            state.settings.account.status = LoginStatus::LoggedOut;
            state.settings.account.error = Some(e);
        }
        SyncMessage::BindScenarioLoaded(Ok(scenario)) => {
            state.settings.account.notice = None;
            state.settings.account.error = None;
            match scenario {
                BindScenario::FreshLocal => {
                    enable_and_run(state, SyncKind::AfterSession).await;
                }
                BindScenario::FreshCloud => {
                    enable_and_run(state, SyncKind::PullOnly).await;
                }
                BindScenario::Conflict(payload) => {
                    open_bind_dialog(state, payload).await;
                }
            }
        }
        SyncMessage::BindScenarioLoaded(Err(e)) => {
            state.settings.account.error = Some(e);
        }
        SyncMessage::CurriculumConflict(payload) => {
            open_bind_dialog(state, payload).await;
            state.toast = Some(Toast::info(labels.account_sync_conflict_toast));
        }
        SyncMessage::SyncFinished(Ok(report)) => {
            let notice = match report.action {
                ReportAction::Sync => labels
                    .account_sync_done
                    .replace("{rev}", &report.revision.to_string()),
                ReportAction::Adopt => labels
                    .account_adopt_done
                    .replace("{count}", &report.topics.unwrap_or(0).to_string()),
                ReportAction::Replace => labels
                    .account_replace_done
                    .replace("{count}", &report.topics.unwrap_or(0).to_string())
                    .replace("{rev}", &report.revision.to_string()),
            };
            let account = &mut state.settings.account;
            account.syncing = false;
            account.notice = Some(notice.clone());
            state.toast = Some(Toast::info(notice));
            refresh_sync_status(state).await;
            if report.action != ReportAction::Sync {
                // Adopt/replace enable sync for the pair as the final step.
                state.settings.account.sync_enabled = true;
            }
        }
        SyncMessage::SyncFinished(Err(failure)) => {
            state.settings.account.syncing = false;
            apply_sync_failure(state, &labels, failure);
        }
        SyncMessage::PullOnStartFinished(Ok(_report)) => {
            refresh_sync_status(state).await;
        }
        SyncMessage::PullOnStartFinished(Err(failure)) => {
            // Offline-first: only a rejected token is surfaced.
            if failure.unauthorized {
                state.settings.account.relogin_required = true;
            }
        }
        SyncMessage::MeFetched(me) => {
            state.settings.account.subscription = me.map(|m| m.subscription_status);
        }
    }
}

fn apply_sync_failure(state: &mut AppState, labels: &SettingsLabels, failure: SyncFailure) {
    let account = &mut state.settings.account;
    if failure.unauthorized {
        account.relogin_required = true;
        account.notice = Some(labels.account_relogin_required.to_string());
    } else if failure.conflict {
        account.notice = Some(labels.account_sync_conflict.to_string());
        state.toast = Some(Toast::info(labels.account_sync_conflict_toast));
    } else {
        account.error = Some(format!(
            "{}: {}",
            labels.account_sync_failed, failure.message
        ));
    }
}

async fn refresh_sync_status(state: &mut AppState) {
    state.settings.account.outbox_len = state.db.outbox().len().await.ok();
    state.settings.account.last_sync_at = state.db.metadata().last_sync_at().await.ok().flatten();
    state.settings.account.sync_enabled = state.db.metadata().sync_enabled().await.unwrap_or(false);
}

/// Enables sync for the pair and starts the given background operation
/// (push for a cloud-empty pair, pull for a locally-empty one).
async fn enable_and_run(state: &mut AppState, kind: SyncKind) {
    let _ = state.db.metadata().set_sync_enabled(true).await;
    state.settings.account.sync_enabled = true;
    state.settings.account.syncing = true;
    spawn_sync(state, kind);
}

/// Opens the conflict-resolution dialog, asking about progress only when
/// there is any locally.
async fn open_bind_dialog(state: &mut AppState, payload: CurriculumPayload) {
    let has_local_progress = state
        .db
        .progress()
        .read_all()
        .await
        .map(|p| !p.topics.is_empty())
        .unwrap_or(false)
        || state
            .db
            .history()
            .read_all()
            .await
            .map(|h| !h.is_empty())
            .unwrap_or(false);
    state.settings.account.bind_dialog = Some(BindDialog {
        step: BindDialogStep::Curriculum,
        payload,
        has_local_progress,
        selected: 0,
    });
}

/// Key handling while the conflict-resolution dialog is open.
pub async fn handle_bind_dialog_key(
    state: &mut AppState,
    code: ratatui::crossterm::event::KeyCode,
) -> Result<()> {
    use ratatui::crossterm::event::KeyCode;
    let Some(dialog) = state.settings.account.bind_dialog.as_mut() else {
        return Ok(());
    };
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            dialog.selected = dialog.selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            dialog.selected = (dialog.selected + 1).min(2);
        }
        KeyCode::Esc => {
            state.settings.account.bind_dialog = None;
        }
        KeyCode::Enter => {
            let dialog = state
                .settings
                .account
                .bind_dialog
                .take()
                .expect("dialog checked above");
            confirm_bind_dialog(state, dialog);
        }
        _ => {}
    }
    Ok(())
}

fn confirm_bind_dialog(state: &mut AppState, dialog: BindDialog) {
    match dialog.step {
        BindDialogStep::Curriculum => match dialog.selected {
            0 => {
                if dialog.has_local_progress {
                    state.settings.account.bind_dialog = Some(BindDialog {
                        step: BindDialogStep::Progress,
                        ..dialog
                    });
                } else {
                    execute_adopt(state, dialog.payload, ProgressMerge::Merge);
                }
            }
            1 => execute_replace(state, dialog.payload),
            // Cancel: nothing changes; sync stays disabled (or the conflict
            // stays unresolved).
            _ => {}
        },
        BindDialogStep::Progress => match dialog.selected {
            0 => execute_adopt(state, dialog.payload, ProgressMerge::Merge),
            1 => execute_adopt(state, dialog.payload, ProgressMerge::StartFromCloud),
            _ => {}
        },
    }
}

fn execute_adopt(state: &mut AppState, payload: CurriculumPayload, merge: ProgressMerge) {
    state.settings.account.syncing = true;
    state.settings.account.notice = None;
    state.settings.account.error = None;
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
        let result = async {
            let store = TokenStore::new(data_dir);
            let token = store
                .load()
                .await
                .map_err(|e| SyncFailure::other(e.to_string()))?
                .ok_or_else(SyncFailure::unauthorized)?;
            let client = SyncClient::new(base_url)
                .map_err(|e| SyncFailure::other(e.to_string()))?
                .with_access_token(token.access_token);
            client
                .adopt_cloud_curriculum(&db, &pair_id, &payload, merge)
                .await
                .map_err(map_sync_err)?;
            let _ = db.metadata().set_sync_enabled(true).await;
            let _ = db
                .metadata()
                .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
                .await;
            let revision = db.metadata().last_pulled_seq().await.unwrap_or(0);
            Ok(SyncReport {
                revision,
                action: ReportAction::Adopt,
                topics: Some(payload.topics.len()),
            })
        }
        .await;
        let _ = tx.send(SyncMessage::SyncFinished(result)).await;
    });
}

fn execute_replace(state: &mut AppState, _payload: CurriculumPayload) {
    state.settings.account.syncing = true;
    state.settings.account.notice = None;
    state.settings.account.error = None;
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
        let result = async {
            let store = TokenStore::new(data_dir);
            let token = store
                .load()
                .await
                .map_err(|e| SyncFailure::other(e.to_string()))?
                .ok_or_else(SyncFailure::unauthorized)?;
            let client = SyncClient::new(base_url)
                .map_err(|e| SyncFailure::other(e.to_string()))?
                .with_access_token(token.access_token);
            let topics = db
                .curriculum()
                .read_all()
                .await
                .map_err(|e| SyncFailure::other(e.to_string()))?
                .topics
                .len();
            let revision = client
                .replace_cloud_curriculum(&db, &pair_id)
                .await
                .map_err(map_push_err)?;
            let _ = db.metadata().set_sync_enabled(true).await;
            let _ = db
                .metadata()
                .set_last_sync_at(&chrono::Utc::now().to_rfc3339())
                .await;
            Ok(SyncReport {
                revision,
                action: ReportAction::Replace,
                topics: Some(topics),
            })
        }
        .await;
        let _ = tx.send(SyncMessage::SyncFinished(result)).await;
    });
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(target_os = "linux")]
    let command = "xdg-open";
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = std::process::Command::new(command).arg(url).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = url;
}

/// Body of the Account section: status, action rows, and the "what is
/// synced" block.
pub fn build_body(state: &AppState, labels: &SettingsLabels) -> Text<'static> {
    let account = &state.settings.account;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let marker = |row: usize| {
        if account.field == row && action_count(account) > 0 {
            "> "
        } else {
            "  "
        }
    };

    match account.status {
        LoginStatus::LoggedOut => {
            lines.push(action_line(marker(0), labels.account_sign_in));
            if account.error.is_none() {
                lines.push(Line::from(""));
                lines.push(Line::from(labels.account_not_logged_in));
            }
        }
        LoginStatus::Starting => {
            lines.push(Line::from(labels.account_starting));
        }
        LoginStatus::WaitingConfirmation => {
            lines.push(Line::from(vec![
                Span::raw(format!("{} ", labels.account_code_label)),
                Span::styled(
                    account.user_code.clone().unwrap_or_default(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(format!(
                "{} {}",
                labels.account_verification_label,
                account.verification_url.clone().unwrap_or_default()
            )));
            lines.push(Line::from(labels.account_waiting));
        }
        LoginStatus::Expired => {
            lines.push(action_line(marker(0), labels.account_sign_in_retry));
            if account.error.is_none() {
                lines.push(Line::from(""));
                lines.push(Line::from(labels.account_expired));
            }
        }
        LoginStatus::LoggingIn => {
            lines.push(Line::from(labels.account_logging_in));
        }
        LoginStatus::LoggedIn => {
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_email_label,
                account.email.as_deref().unwrap_or("—")
            )));
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_device_label,
                account.device_id.as_deref().unwrap_or("—")
            )));
            let backend = match account.token_backend {
                Some(TokenBackend::Keychain) => labels.account_token_keychain,
                Some(TokenBackend::File) => labels.account_token_file,
                None => "—",
            };
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_token_label, backend
            )));
            if account.token_backend == Some(TokenBackend::File) {
                lines.push(Line::from(Span::styled(
                    labels.account_token_file_warning,
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_subscription_label,
                account
                    .subscription
                    .as_deref()
                    .unwrap_or(labels.account_unavailable)
            )));
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_last_sync_label,
                account
                    .last_sync_at
                    .as_deref()
                    .unwrap_or(labels.account_never_synced)
            )));
            lines.push(Line::from(format!(
                "{}: {}",
                labels.account_pending_label,
                account
                    .outbox_len
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string())
            )));
            if account.relogin_required {
                lines.push(Line::from(Span::styled(
                    labels.account_relogin_required,
                    Style::default().fg(Color::Red),
                )));
            }
            lines.push(Line::from(""));
            let sync_now = if account.syncing {
                labels.account_syncing
            } else {
                labels.account_sync_now
            };
            lines.push(action_line(marker(0), sync_now));
            let toggle = if account.sync_enabled {
                labels.account_sync_on
            } else {
                labels.account_sync_off
            };
            lines.push(Line::from(format!(
                "{}{}: {}",
                marker(1),
                labels.account_sync_toggle_label,
                toggle
            )));
            lines.push(action_line(marker(2), labels.account_sign_out));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                labels.account_sync_block_title,
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(labels.account_synced_items));
            lines.push(Line::from(labels.account_not_synced_items));
        }
    }

    if account.syncing && account.status != LoginStatus::LoggedIn {
        lines.push(Line::from(labels.account_syncing));
    }
    if let Some(error) = &account.error {
        lines.push(Line::from(""));
        lines.extend(error_lines(error));
    }
    if let Some(notice) = &account.notice {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            notice.clone(),
            Style::default().fg(Color::Yellow),
        )));
    }

    Text::from(lines)
}

fn action_line<'a>(marker: &'a str, label: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::raw(marker),
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
    ])
}

/// Renders the conflict-resolution dialog as a centered modal box.
pub fn draw_bind_dialog(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &AppState) {
    use ratatui::layout::{Alignment, Constraint, Direction, Layout};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

    let Some(dialog) = &state.settings.account.bind_dialog else {
        return;
    };
    let lang = crate::ui::labels::native_language_code(state.config.as_ref());
    let labels = crate::ui::labels::get_settings_labels(lang);
    let common = crate::ui::labels::get_common_labels(lang);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(area);
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(vertical[1])[1];

    let (title, options) = match dialog.step {
        BindDialogStep::Curriculum => (
            labels.account_conflict_title,
            vec![
                labels.account_conflict_adopt,
                labels.account_conflict_replace,
                labels.account_conflict_cancel,
            ],
        ),
        BindDialogStep::Progress => (
            labels.account_progress_merge_title,
            vec![
                labels.account_progress_merge_merge,
                labels.account_progress_merge_cloud,
                labels.account_conflict_cancel,
            ],
        ),
    };

    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(crate::ui::colors::BLUE))
        .title_style(
            Style::default()
                .fg(crate::ui::colors::BLUE)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let mut lines = Vec::new();
    if dialog.step == BindDialogStep::Curriculum {
        lines.push(Line::from(
            labels
                .account_conflict_topics
                .replace("{count}", &dialog.payload.topics.len().to_string()),
        ));
        lines.push(Line::from(""));
    }
    for (i, option) in options.iter().enumerate() {
        let marker = if i == dialog.selected { "> " } else { "  " };
        lines.push(Line::from(vec![
            Span::raw(marker),
            Span::styled(*option, Style::default().add_modifier(Modifier::BOLD)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }),
        chunks[0],
    );

    let footer = Line::from(Span::styled(
        format!(
            "↑/↓ {} | Enter {} | Esc {}",
            common.navigate, common.confirm, common.back
        ),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        chunks[1],
    );
}
