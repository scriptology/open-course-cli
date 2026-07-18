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
    AppState::new(
        PathBuf::from(dir.path()),
        Arc::new(db),
        Some(make_test_config()),
        Arc::new(AtomicBool::new(false)),
        tx,
    )
    .unwrap()
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
    assert!(text.contains("Age"), "Profile should show Age");
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
        text.contains("Batch size"),
        "Session should show Batch size"
    );
    assert!(
        text.contains("recommended"),
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
        text.contains("Reset progress"),
        "Data should show reset actions"
    );
    assert!(text.contains("Reset all"), "Data should show Reset all");
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
    assert_eq!(
        state.settings.success,
        Some("Saved".to_string()),
        "Enter should show a success message"
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
        text.contains("Update available"),
        "Prompt should show update title"
    );
    assert!(
        text.contains("Latest: v9.9.9"),
        "Prompt should show latest version"
    );
    assert!(
        text.contains("y: install"),
        "Prompt should offer install action"
    );
    assert!(text.contains("n: skip"), "Prompt should offer skip action");
}
