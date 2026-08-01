use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use open_course_cli::app::{AppState, View};
use open_course_cli::config::profile::{UserPreferences, UserProfile};
use open_course_cli::config::{OpenCourseConfig, ProviderConfig, ProviderId};
use open_course_cli::db::Database;
use open_course_cli::db::curriculum::Topic;
use open_course_cli::ui::views::{curriculum, dashboard, docs};
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

fn make_topic(id: &str, name: &str) -> Topic {
    Topic {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        difficulty: "beginner".to_string(),
        level: Some("A1".to_string()),
        order: None,
        tags: Vec::new(),
        target_lang: "en".to_string(),
        native_lang: "ru".to_string(),
        version: 1,
        ..Default::default()
    }
}

async fn setup_state() -> AppState {
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

    let dir = tempfile::TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    AppState::new(
        PathBuf::from(dir.path()),
        Arc::new(db),
        Some(config),
        Arc::new(AtomicBool::new(false)),
        tx,
    )
    .unwrap()
}

#[tokio::test]
async fn dashboard_hint_bar_shows_start_first_without_docs() {
    let mut state = setup_state().await;
    state.view = View::Dashboard;

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| dashboard::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(text.contains("n:Начать"), "hint bar should show n: Начать");
    assert!(
        !text.contains("Документация"),
        "hint bar should not advertise docs anymore"
    );
    assert!(
        !text.contains("Выбор темы"),
        "hint bar should hide topic selection"
    );
    assert!(
        !text.contains("колесо") && !text.contains("m: выделение"),
        "hint bar should hide wheel/m hints"
    );
}

#[tokio::test]
async fn curriculum_footer_shows_enter_practice_and_d_docs() {
    let mut state = setup_state().await;
    state.view = View::Curriculum;
    state.curriculum.topics = vec![make_topic("t1", "Preterito")];

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| curriculum::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Enter: Начать практику"),
        "Enter should start practice, got:\n{text}"
    );
    assert!(
        text.contains("d: Документация"),
        "d should open documentation, got:\n{text}"
    );
    assert!(
        !text.contains("колесо"),
        "curriculum footer should hide wheel/m hints"
    );
}

#[tokio::test]
async fn docs_detail_footer_uses_all_topics_and_start_practice() {
    let mut state = setup_state().await;
    state.view = View::Docs;
    state.docs.viewing_topic = Some(make_topic("t1", "Preterito"));
    state.docs.content = "Some review content".to_string();

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| docs::draw(f, f.area(), &mut state))
        .unwrap();

    let text = buffer_text(&terminal);
    assert!(
        text.contains("Esc: Все темы"),
        "Esc should be labeled Все темы, got:\n{text}"
    );
    assert!(
        text.contains("n: Начать практику"),
        "practice should be on n, got:\n{text}"
    );
    assert!(
        !text.contains("скролл"),
        "scroll hint should be hidden, got:\n{text}"
    );
}
