use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use open_course_cli::app::{AppState, View};
use open_course_cli::db::Database;
use open_course_cli::ui::views::dashboard;
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

#[tokio::test]
#[ignore = "layout inspection helper, prints buffers"]
async fn render_dashboard_at_various_sizes() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Database::connect(&dir.path().join("db")).await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let mut state = AppState::new(
        PathBuf::from("."),
        Arc::new(db),
        None,
        Arc::new(AtomicBool::new(false)),
        tx,
    )
    .unwrap();
    state.view = View::Dashboard;

    for (w, h) in [(80u16, 24u16), (80, 20), (80, 30), (120, 40)] {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| dashboard::draw(f, f.area(), &mut state))
            .unwrap();
        println!("=== {w}x{h} (top) ===");
        println!("{}", buffer_text(&terminal));

        if state.dashboard.max_scroll > 0 {
            state.dashboard.scroll_by(3);
            terminal
                .draw(|f| dashboard::draw(f, f.area(), &mut state))
                .unwrap();
            println!("=== {w}x{h} (scrolled +3) ===");
            println!("{}", buffer_text(&terminal));

            state.dashboard.scroll_by(100);
            terminal
                .draw(|f| dashboard::draw(f, f.area(), &mut state))
                .unwrap();
            println!("=== {w}x{h} (scrolled to bottom) ===");
            println!("{}", buffer_text(&terminal));
        }
        state.dashboard.scroll_offset = 0;
    }
}
