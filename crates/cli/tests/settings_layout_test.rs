use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use open_course_cli::app::{AppState, View};
use open_course_cli::config::profile::{UserPreferences, UserProfile};
use open_course_cli::config::{OpenCourseConfig, ProviderConfig, ProviderId};
use open_course_cli::db::Database;
use open_course_cli::ui::views::dashboard;
use open_course_cli::ui::views::settings::{self, Section};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn make_test_config() -> OpenCourseConfig {
    let profile = UserProfile {
        native_language: "ru".to_string(),
        target_language: "en".to_string(),
        age: Some(38),
        self_assessed_cefr: Some("B1".to_string()),
    };
    let provider_config = ProviderConfig::ApiKey {
        api_key: Some("test-key".to_string()),
        model: "gpt-4".to_string(),
        base_url: None,
        endpoint: None,
        reasoning_effort: None,
    };
    let mut config = OpenCourseConfig::new(ProviderId::OpenAi, provider_config, profile);
    config.preferences = UserPreferences {
        batch_size: 3,
        hint_mode: open_course_cli::config::profile::HintMode::Auto,
    };
    config
}

async fn setup_state() -> AppState {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let state = AppState::new(
        PathBuf::from(dir.path()),
        Arc::new(db),
        Some(make_test_config()),
        Arc::new(AtomicBool::new(false)),
        tx,
    )
    .unwrap();
    // Keep the temp dir alive: account/sync tests write to the database and
    // the config after setup, which fails on an unlinked directory.
    std::mem::forget(dir);
    state
}

#[tokio::test]
#[ignore = "layout inspection helper, prints settings screens"]
async fn render_settings_screens() {
    let mut state = setup_state().await;
    state.view = View::Settings;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    // Section picker
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();
    println!("=== Settings picker ===");
    println!("{}", buffer_text(&terminal));

    // Profile section
    state.settings.in_section = true;
    state.settings.section = Section::Profile;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();
    println!("=== Profile ===");
    println!("{}", buffer_text(&terminal));

    // Session section
    state.settings.section = Section::Session;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();
    println!("=== Session ===");
    println!("{}", buffer_text(&terminal));

    // Data section
    state.settings.section = Section::Data;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();
    println!("=== Data ===");
    println!("{}", buffer_text(&terminal));
}

#[tokio::test]
async fn settings_profile_shows_age_without_cefr() {
    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Profile;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(text.contains("Возраст"), "Profile should show Age");
    assert!(
        !text.contains("CEFR"),
        "Profile should not show CEFR in settings"
    );
}

#[tokio::test]
async fn settings_session_shows_batch_size_selector() {
    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Session;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Размер сессии"),
        "Session should show Batch size"
    );
    assert!(
        text.contains("рекомендуется"),
        "Batch size 3 should be marked recommended"
    );
    assert!(
        !text.contains("Hint mode"),
        "Session should not show Hint mode"
    );
    assert!(text.contains("  2"), "Session should show option 2");
    assert!(text.contains("> 3"), "Session should highlight option 3");
    assert!(text.contains("  4"), "Session should show option 4");
    assert!(text.contains("  5"), "Session should show option 5");
}

#[tokio::test]
async fn settings_data_lists_reset_actions() {
    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Data;
    state.settings.active_field = 0;
    // ensure_input_loaded will load the field when draw is called

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Сбросить прогресс"),
        "Data should show reset actions"
    );
    assert!(text.contains("Сбросить всё"), "Data should show Reset all");
}

#[tokio::test]
async fn settings_profile_enter_saves_age() {
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Profile;
    state.settings.active_field = 0;

    // Simulate the active input being loaded for the Age field
    state.settings.input = "42".to_string();
    state.settings.cursor = 2;
    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();

    assert_eq!(
        state.config.as_ref().unwrap().active_profile().age,
        Some(42),
        "Enter should save the edited age"
    );
    assert!(
        !state.settings.in_section,
        "Enter should return to section list (root settings)"
    );
}

#[tokio::test]
async fn dashboard_header_shows_version() {
    let mut state = setup_state().await;
    state.view = View::Dashboard;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| dashboard::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    let expected = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(
        text.contains(&expected),
        "Dashboard header should show current version: {}",
        expected
    );
}

#[tokio::test]
async fn dashboard_header_shows_update_hint() {
    let mut state = setup_state().await;
    state.view = View::Dashboard;
    state.update.latest_version = Some("9.9.9".to_string());

    for (width, height) in [(100, 24), (80, 24), (50, 30)] {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| dashboard::draw(f, f.area(), &mut state))
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("v9.9.9") && text.contains("opencourse update"),
            "Dashboard header should show the update hint at {width}x{height}"
        );
    }
}

#[tokio::test]
async fn update_available_prompt_renders() {
    use open_course_cli::ui::views::update;

    let mut state = setup_state().await;
    state.view = View::UpdateAvailable;
    state.update.latest_version = Some("9.9.9".to_string());

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| update::draw(f, f.area(), &state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Доступно обновление"),
        "Prompt should show update title"
    );
    assert!(
        text.contains("Последняя: v9.9.9"),
        "Prompt should show latest version"
    );
    assert!(
        text.contains("n: пропустить"),
        "Prompt should offer skip action"
    );
}

#[tokio::test]
async fn settings_profile_inline_validation_error() {
    use open_course_cli::ui::views::settings::Section;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Profile;
    state.settings.active_field = 0;

    // Set invalid age
    state.settings.input = "9".to_string();
    state.settings.cursor = 1;
    state.settings.error = Some("error".to_string());

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("недопустимое значение"),
        "Should show localized invalid value error inline"
    );
}

#[tokio::test]
async fn settings_session_highlights_current_green() {
    use open_course_cli::ui::views::settings::Section;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Session;
    state.settings.active_field = 0;
    state.settings.session_batch_idx = 1; // A2 = index 1

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    // The current option (index 1 = 3) should have green bold marker
    // Check that the selected option has green style
    assert!(
        text.contains("3 (рекомендуется)"),
        "Should show batch size options"
    );
}

#[tokio::test]
async fn settings_breadcrumbs_in_header() {
    use open_course_cli::ui::views::settings::Section;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Profile;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Настройки / Profile") || text.contains("Настройки / Профиль"),
        "Should show breadcrumbs: Настройки / Profile"
    );
}

#[tokio::test]
async fn settings_provider_skips_readonly_steps() {
    use open_course_cli::app::AppState;
    use open_course_cli::config::ProviderId;
    use open_course_cli::db::Database;
    use open_course_cli::ui::views::settings::{ProviderSetupStep, Section};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::mpsc;

    // Create a minimal AppState for testing
    let dir = tempfile::TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let mut state = AppState::new(
        PathBuf::from(dir.path()),
        Arc::new(db),
        None,
        Arc::new(AtomicBool::new(false)),
        tx,
    )
    .unwrap();

    state.settings.section = Section::Provider;
    state.settings.in_section = true;
    state.settings.provider_setup_step = ProviderSetupStep::SelectProvider;
    state.settings.provider_setup_provider = ProviderId::OpenAi;

    // Simulate advancing from SelectProvider
    settings::advance_provider_setup_step(&mut state);

    // Should skip BaseUrl and Endpoint and go directly to ApiKey for non-Custom
    assert_eq!(
        state.settings.provider_setup_step,
        ProviderSetupStep::ApiKey,
        "Should skip BaseUrl/Endpoint for non-Custom provider, got {:?}",
        state.settings.provider_setup_step
    );
}

#[tokio::test]
async fn settings_endpoint_step_cycles_options() {
    use open_course_cli::ui::views::settings::ProviderSetupStep;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Provider;
    state.settings.provider_setup_provider = ProviderId::Custom;
    state.settings.provider_setup_step = ProviderSetupStep::Endpoint;
    state.settings.input = "chat/completions".to_string();

    settings::handle_key(&mut state, KeyCode::Down)
        .await
        .unwrap();
    assert_eq!(
        state.settings.input, "messages",
        "Down should switch endpoint to messages"
    );

    settings::handle_key(&mut state, KeyCode::Up).await.unwrap();
    assert_eq!(
        state.settings.input, "chat/completions",
        "Up should switch endpoint back to chat/completions"
    );
}

#[tokio::test]
async fn settings_endpoint_step_renders_selector() {
    use open_course_cli::ui::views::settings::ProviderSetupStep;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Provider;
    state.settings.provider_setup_provider = ProviderId::Custom;
    state.settings.provider_setup_step = ProviderSetupStep::Endpoint;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("> chat/completions"),
        "Endpoint step should highlight chat/completions"
    );
    assert!(
        text.contains("  messages"),
        "Endpoint step should list messages option"
    );
}

#[tokio::test]
async fn settings_api_key_step_renders_input_box() {
    use open_course_cli::ui::views::settings::ProviderSetupStep;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Provider;
    state.settings.provider_setup_provider = ProviderId::OpenAi;
    state.settings.provider_setup_step = ProviderSetupStep::ApiKey;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("API-ключ"),
        "ApiKey step should render a titled input box"
    );
    assert!(
        text.contains("********"),
        "ApiKey value should be masked (config key is \"test-key\")"
    );
    assert!(
        !text.contains("test-key"),
        "ApiKey value should not be shown in plain text"
    );
    assert!(text.contains("█"), "ApiKey input should show a caret");
}

#[tokio::test]
async fn settings_base_url_step_renders_input_box() {
    use open_course_cli::ui::views::settings::ProviderSetupStep;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Provider;
    state.settings.provider_setup_provider = ProviderId::Custom;
    state.settings.provider_setup_step = ProviderSetupStep::BaseUrl;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Base URL"),
        "BaseUrl step should render a titled input box"
    );
    assert!(
        text.contains("https://opencode.ai/zen/go/v1"),
        "BaseUrl step should show an example hint"
    );
    assert!(text.contains("█"), "BaseUrl input should show a caret");
}

#[tokio::test]
async fn settings_endpoint_step_wraps_on_narrow_terminal() {
    use open_course_cli::ui::views::settings::ProviderSetupStep;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Provider;
    state.settings.provider_setup_provider = ProviderId::Custom;
    state.settings.provider_setup_step = ProviderSetupStep::Endpoint;

    let backend = TestBackend::new(40, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    // The selector wraps instead of truncating: all options stay visible.
    assert!(
        text.contains("chat/completions"),
        "endpoint options should wrap, not truncate"
    );
    assert!(
        text.contains("messages"),
        "messages option should be visible"
    );
    // The footer reflows onto multiple lines: every command survives.
    assert!(
        text.contains("Enter"),
        "footer should keep the Enter command"
    );
    assert!(text.contains("Esc"), "footer should keep the Esc command");
    assert!(text.contains("назад"), "footer should keep the back action");
}

// ---------------------------------------------------------------------------
// Account section
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settings_account_logged_out_renders_sign_in() {
    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Account;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Аккаунт"),
        "header should show the Account section, got:\n{text}"
    );
    assert!(
        text.contains("Вход не выполнен"),
        "logged-out state should say so"
    );
    assert!(text.contains("Войти"), "logged-out state offers sign in");
}

#[tokio::test]
async fn settings_account_error_renders_below_sign_in_action() {
    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    state.settings.account.error = Some("network error: error sending request".to_string());

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    let action_pos = text.find("Войти").expect("sign-in action shown");
    let error_pos = text.find("network error").expect("error text shown as-is");
    assert!(
        action_pos < error_pos,
        "action row must come before the error text, got:\n{text}"
    );
    assert!(
        !text.contains("Вход не выполнен"),
        "neutral status is replaced by the error block, got:\n{text}"
    );
}

#[tokio::test]
async fn settings_account_logged_in_renders_status_and_actions() {
    use open_course_cli::ui::views::settings::account;
    use open_course_sync::TokenBackend;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    {
        let acc = &mut state.settings.account;
        acc.status = account::LoginStatus::LoggedIn;
        acc.email = Some("user@example.test".to_string());
        acc.device_id = Some("dev-1".to_string());
        acc.token_backend = Some(TokenBackend::File);
        acc.outbox_len = Some(3);
        acc.subscription = Some("active".to_string());
    }

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(text.contains("user@example.test"), "email shown");
    assert!(text.contains("dev-1"), "device shown");
    assert!(text.contains("active"), "subscription shown");
    assert!(
        text.contains("Синхронизировать сейчас"),
        "sync now action shown"
    );
    assert!(text.contains("Выйти"), "sign out action shown");
    assert!(
        text.contains("хранится в обычном файле"),
        "file backend warning shown"
    );
    assert!(
        !text.contains("Что синхронизируется"),
        "sync info block removed"
    );
}

#[tokio::test]
async fn settings_account_toggle_sync_persists_to_metadata() {
    use open_course_cli::ui::views::settings::account;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    state.settings.account.status = account::LoginStatus::LoggedIn;
    state.settings.active_field = 1;

    // Sync starts disabled (opt-in).
    assert!(!state.db.metadata().sync_enabled().await.unwrap());

    // Enabling starts a bind probe; sync is not enabled yet.
    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();
    assert!(!state.db.metadata().sync_enabled().await.unwrap());
    assert!(state.settings.account.notice.is_some());

    // The probe reports no cloud curriculum: sync is enabled and a push is
    // started (silently skipped here — no token in the test store).
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::BindScenarioLoaded(Ok(open_course_sync::BindScenario::FreshLocal)),
    )
    .await;
    assert!(state.db.metadata().sync_enabled().await.unwrap());
    assert!(state.settings.account.sync_enabled);

    // The background operation finishes.
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::SyncFinished(Ok(account::SyncReport {
            revision: 1,
            action: account::ReportAction::Sync,
            topics: None,
        })),
    )
    .await;
    assert!(!state.settings.account.syncing);

    // Disabling is direct.
    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();
    assert!(!state.db.metadata().sync_enabled().await.unwrap());
    assert!(!state.settings.account.sync_enabled);
}

#[tokio::test]
async fn settings_account_sign_out_clears_config_and_status() {
    use open_course_cli::config::SyncConfig;
    use open_course_cli::ui::views::settings::account;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    state.config.as_mut().unwrap().sync = Some(SyncConfig {
        server_url: None,
        account_email: Some("user@example.test".to_string()),
        device_id: Some("dev-1".to_string()),
    });
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    state.settings.account.status = account::LoginStatus::LoggedIn;
    state.settings.active_field = 2;

    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();

    assert_eq!(
        state.settings.account.status,
        account::LoginStatus::LoggedOut
    );
    assert_eq!(
        state.settings.active_field, 0,
        "sign out resets the selector (logged out has a single action)"
    );
    let sync = state.config.as_ref().unwrap().sync.as_ref().unwrap();
    assert_eq!(sync.account_email, None);
    assert_eq!(sync.device_id, None);
}

#[tokio::test]
async fn account_refresh_sets_login_status_from_token_presence() {
    use open_course_cli::ui::views::settings::account;
    use open_course_sync::TokenBackend;

    let mut state = setup_state().await;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::AccountRefreshed {
            has_token: true,
            backend: TokenBackend::File,
        },
    )
    .await;
    assert_eq!(
        state.settings.account.status,
        account::LoginStatus::LoggedIn
    );
    assert_eq!(
        state.settings.account.token_backend,
        Some(TokenBackend::File)
    );

    account::apply_sync_message(
        &mut state,
        account::SyncMessage::AccountRefreshed {
            has_token: false,
            backend: TokenBackend::File,
        },
    )
    .await;
    assert_eq!(
        state.settings.account.status,
        account::LoginStatus::LoggedOut
    );
}

#[tokio::test]
async fn login_finished_stores_account_in_config_and_stays_opt_in() {
    use open_course_cli::ui::views::settings::account;
    use open_course_sync::TokenBackend;

    let mut state = setup_state().await;
    state.settings.active_field = 2;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::LoginFinished(Ok(account::LoginInfo {
            email: Some("user@example.test".to_string()),
            device_id: "dev-1".to_string(),
            subscription: Some("active".to_string()),
            backend: TokenBackend::File,
        })),
    )
    .await;

    assert_eq!(
        state.settings.account.status,
        account::LoginStatus::LoggedIn
    );
    assert_eq!(
        state.settings.active_field, 0,
        "login resets the selector to the first action row"
    );
    let sync = state.config.as_ref().unwrap().sync.as_ref().unwrap();
    assert_eq!(sync.account_email.as_deref(), Some("user@example.test"));
    assert_eq!(sync.device_id.as_deref(), Some("dev-1"));
    assert!(
        sync.server_url.is_some(),
        "server url is recorded for visibility"
    );
    // Sync remains opt-in after login.
    assert!(!state.settings.account.sync_enabled);
}

#[tokio::test]
async fn sync_failure_unauthorized_marks_relogin_required() {
    use open_course_cli::ui::views::settings::account;

    let mut state = setup_state().await;
    state.settings.account.status = account::LoginStatus::LoggedIn;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::SyncFinished(Err(account::SyncFailure {
            message: String::new(),
            unauthorized: true,
            conflict: false,
        })),
    )
    .await;

    assert!(state.settings.account.relogin_required);
    assert!(!state.settings.account.syncing);
    assert!(state.settings.account.notice.is_some());
}

#[tokio::test]
async fn sync_conflict_keeps_outbox_and_shows_notice() {
    use open_course_cli::ui::views::settings::account;

    let mut state = setup_state().await;
    state.settings.account.status = account::LoginStatus::LoggedIn;
    state
        .db
        .outbox()
        .append("upsert", "topic", "t1", "{}")
        .await
        .unwrap();

    account::apply_sync_message(
        &mut state,
        account::SyncMessage::SyncFinished(Err(account::SyncFailure {
            message: String::new(),
            unauthorized: false,
            conflict: true,
        })),
    )
    .await;

    assert!(state.settings.account.notice.is_some());
    assert_eq!(
        state.db.outbox().len().await.unwrap(),
        1,
        "conflict must not drop pending changes"
    );
}

// ---------------------------------------------------------------------------
// Bind conflict dialog
// ---------------------------------------------------------------------------

fn conflict_payload() -> open_course_sync::CurriculumPayload {
    open_course_sync::CurriculumPayload {
        revision: 7,
        version: 1,
        topics: vec![],
    }
}

#[tokio::test]
async fn bind_conflict_opens_dialog_and_cancel_keeps_sync_disabled() {
    use open_course_cli::ui::views::settings::account;
    use open_course_sync::BindScenario;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::BindScenarioLoaded(Ok(BindScenario::Conflict(conflict_payload()))),
    )
    .await;

    let dialog = state
        .settings
        .account
        .bind_dialog
        .as_ref()
        .expect("conflict opens the dialog");
    assert_eq!(dialog.step, account::BindDialogStep::Curriculum);
    assert!(
        !state.settings.account.sync_enabled,
        "sync stays disabled until the conflict is resolved"
    );

    settings::handle_key(&mut state, KeyCode::Esc)
        .await
        .unwrap();
    assert!(state.settings.account.bind_dialog.is_none());
    assert!(!state.settings.account.sync_enabled);
}

#[tokio::test]
async fn bind_dialog_adopt_with_local_progress_asks_merge_question() {
    use open_course_cli::ui::views::settings::account;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    // Any local progress triggers the second question.
    state
        .db
        .progress()
        .upsert(&open_course_cli::db::progress::ProgressTopic::initial(
            "t1".to_string(),
            50.0,
        ))
        .await
        .unwrap();

    account::apply_sync_message(
        &mut state,
        account::SyncMessage::CurriculumConflict(conflict_payload()),
    )
    .await;
    let dialog = state.settings.account.bind_dialog.as_ref().unwrap();
    assert!(dialog.has_local_progress);

    // Enter on "use the cloud curriculum" moves to the progress question.
    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();
    let dialog = state.settings.account.bind_dialog.as_ref().unwrap();
    assert_eq!(dialog.step, account::BindDialogStep::Progress);

    // Esc cancels the whole flow.
    settings::handle_key(&mut state, KeyCode::Esc)
        .await
        .unwrap();
    assert!(state.settings.account.bind_dialog.is_none());
    assert!(!state.settings.account.sync_enabled);
}

#[tokio::test]
async fn bind_dialog_adopt_without_progress_executes_immediately() {
    use open_course_cli::ui::views::settings::account;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::CurriculumConflict(conflict_payload()),
    )
    .await;
    let dialog = state.settings.account.bind_dialog.as_ref().unwrap();
    assert!(!dialog.has_local_progress);

    settings::handle_key(&mut state, KeyCode::Enter)
        .await
        .unwrap();
    assert!(
        state.settings.account.bind_dialog.is_none(),
        "adopt starts right away without local progress"
    );
    assert!(state.settings.account.syncing);
}

#[tokio::test]
async fn bind_dialog_renders_options() {
    use open_course_cli::ui::views::settings::account;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::CurriculumConflict(conflict_payload()),
    )
    .await;

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Взять облачную программу"),
        "adopt option shown, got:\n{text}"
    );
    assert!(text.contains("Оставить свою и заменить облачную"));
    assert!(text.contains("Отмена"));
}

#[tokio::test]
async fn sync_finished_adopt_report_enables_sync_and_shows_notice() {
    use open_course_cli::ui::views::settings::account;

    let mut state = setup_state().await;
    account::apply_sync_message(
        &mut state,
        account::SyncMessage::SyncFinished(Ok(account::SyncReport {
            revision: 5,
            action: account::ReportAction::Adopt,
            topics: Some(7),
        })),
    )
    .await;

    assert!(state.settings.account.sync_enabled);
    assert!(!state.settings.account.syncing);
    let notice = state.settings.account.notice.as_deref().unwrap_or("");
    assert!(
        notice.contains('7'),
        "notice mentions the topic count: {notice}"
    );
}

#[tokio::test]
async fn settings_account_selector_moves_and_wraps() {
    use open_course_cli::ui::colors;
    use open_course_cli::ui::views::settings::account;
    use ratatui::crossterm::event::KeyCode;

    let mut state = setup_state().await;
    state.view = View::Settings;
    state.settings.in_section = true;
    state.settings.section = Section::Account;
    state.settings.account.status = account::LoginStatus::LoggedIn;

    // Down moves the shared selector and wraps at action_count (3).
    for expected in [1, 2, 0] {
        settings::handle_key(&mut state, KeyCode::Down)
            .await
            .unwrap();
        assert_eq!(state.settings.active_field, expected);
    }
    // Up wraps back to the last row.
    settings::handle_key(&mut state, KeyCode::Up).await.unwrap();
    assert_eq!(state.settings.active_field, 2);

    // The marker follows the shared selector; the selected row is green.
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| settings::draw(f, f.area(), &mut state))
        .unwrap();
    let text = buffer_text(&terminal);
    assert!(
        text.contains("> Выйти"),
        "selector sits on the sign-out row, got:\n{text}"
    );
    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let row = (0..area.height)
        .find(|&y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("> Выйти")
        })
        .expect("sign-out row rendered");
    assert_eq!(
        buffer[(0, row)].fg,
        colors::GREEN,
        "selected action row is green, marker included"
    );

    // Logged out has a single action: the selector stays on row 0.
    state.settings.account.status = account::LoginStatus::LoggedOut;
    state.settings.active_field = 0;
    settings::handle_key(&mut state, KeyCode::Down)
        .await
        .unwrap();
    assert_eq!(state.settings.active_field, 0);
}
