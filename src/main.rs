use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use ratatui::crossterm::{
    cursor::MoveTo,
    event::DisableMouseCapture,
    execute,
    terminal::{Clear, ClearType},
};

use open_course_cli::app::run_app;
use open_course_cli::config;
use open_course_cli::db::Database;
use open_course_cli::db::curriculum::cleanup_topics;
use open_course_cli::llm::pipeline::log_debug_event;

#[derive(Parser)]
#[command(name = "open-course-cli", version, about = "AI language learning terminal")]
struct Cli {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long = "data-dir")]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = cli.cwd.canonicalize()?;
    let data_dir = cli.data_dir.unwrap_or_else(|| cwd.clone());

    let config = config::read_config(&data_dir)?;

    let db = if let Some(ref cfg) = config {
        let db_path = config::pair_db_path(&data_dir, &cfg.active_pair);
        if config::migration::should_recreate_curriculum_table(&data_dir) {
            Database::recreate_curriculum_table(&db_path).await?;
            config::migration::mark_curriculum_table_recreated(&data_dir)?;
        }
        let db = Database::connect(&db_path).await?;
        if let Some(curriculum) = config::migration::try_migrate_from_curriculum_md(
            &data_dir,
        )? {
            let table = db.curriculum();
            for topic in &curriculum.topics {
                table.upsert(topic).await?;
            }
        }
        if config::migration::should_clear_reviews_cache(&data_dir) {
            db.reviews().reset().await?;
            config::migration::mark_reviews_cache_cleared(&data_dir)?;
        }
        let (moved, removed) = cleanup_topics(&db).await?;
        if moved > 0 || removed > 0 {
            eprintln!("Cleaned up {moved} micro-topics and removed {removed} stale topics");
        }
        Arc::new(db)
    } else {
        let fallback_db = config::open_course_dir(&data_dir).join("db");
        let db = Database::connect(&fallback_db).await?;
        let (moved, removed) = cleanup_topics(&db).await?;
        if moved > 0 || removed > 0 {
            eprintln!("Cleaned up {moved} micro-topics and removed {removed} stale topics");
        }
        Arc::new(db)
    };

    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_some() {
        log_debug_event(
            "startup",
            &format!("OPEN_COURSE_CLI_DEBUG enabled. data_dir: {}", data_dir.display()),
            Some(&data_dir),
        );
    }

    config::ensure_open_course_gitignore(&data_dir)?;

    setup_panic_hook();

    let quit = Arc::new(AtomicBool::new(false));
    let quit_for_signal = quit.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            quit_for_signal.store(true, Ordering::Relaxed);
        }
    });

    let mut stdout = std::io::stdout();
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;

    let mut terminal = ratatui::init();
    terminal.clear()?;
    let result = run_app(&mut terminal, data_dir, db, config, quit).await;
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    let _ = terminal.clear();
    ratatui::restore();
    println!();
    result?;

    Ok(())
}

fn setup_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        println!();
        original(info);
    }));
}
