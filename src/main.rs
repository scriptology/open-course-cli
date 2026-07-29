use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, Subcommand};
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
use open_course_cli::ui::labels::native_language_code;
use open_course_cli::update;

#[derive(Parser)]
#[command(name = "opencourse", version, about = "AI language learning terminal")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Use this directory's `.open-course-cli` for data instead of the global
    /// `~/.open-course-cli`.
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long = "data-dir")]
    data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Check for a newer release and install it.
    Update,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Update) = cli.command {
        return run_update().await;
    }

    let data_dir = match (cli.data_dir, cli.cwd) {
        (Some(dir), _) => dir,
        (None, Some(cwd)) => cwd.canonicalize()?,
        (None, None) => {
            let cwd = std::env::current_dir()?.canonicalize()?;
            config::resolve_data_dir(&cwd)
        }
    };

    let config = config::read_config(&data_dir)?;
    let lang = native_language_code(config.as_ref()).to_string();

    let db = if let Some(ref cfg) = config {
        let db_path = config::pair_db_path(&data_dir, &cfg.active_pair);
        if config::migration::should_recreate_curriculum_table(&data_dir) {
            Database::recreate_curriculum_table(&db_path).await?;
            config::migration::mark_curriculum_table_recreated(&data_dir)?;
        }
        let db = Database::connect(&db_path).await?;
        if let Some(curriculum) = config::migration::try_migrate_from_curriculum_md(&data_dir)? {
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
            eprintln!("{}", cleanup_message(&lang, moved, removed));
        }
        let (removed_items, removed_topics) = dedupe_tables(&db).await?;
        if removed_items > 0 || removed_topics > 0 {
            eprintln!("{}", dedupe_message(&lang, removed_items, removed_topics));
        }
        Arc::new(db)
    } else {
        let fallback_db = config::open_course_dir(&data_dir).join("db");
        let db = Database::connect(&fallback_db).await?;
        let (moved, removed) = cleanup_topics(&db).await?;
        if moved > 0 || removed > 0 {
            eprintln!("{}", cleanup_message(&lang, moved, removed));
        }
        let (removed_items, removed_topics) = dedupe_tables(&db).await?;
        if removed_items > 0 || removed_topics > 0 {
            eprintln!("{}", dedupe_message(&lang, removed_items, removed_topics));
        }
        Arc::new(db)
    };

    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_some() {
        log_debug_event(
            "startup",
            &format!(
                "OPEN_COURSE_CLI_DEBUG enabled. data_dir: {}",
                data_dir.display()
            ),
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

/// `opencourse update`: check GitHub for a newer release and install it via
/// the same installer script the in-app prompt uses.
async fn run_update() -> anyhow::Result<()> {
    // The config is not loaded for the `update` subcommand, so CLI messages
    // fall back to English.
    let lang = "en";
    let latest = update::latest_release_version().await?;

    match latest {
        Some(latest) if update::is_newer(update::CURRENT_VERSION, &latest) => {
            println!("{}", updating_message(lang, &latest));
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(update::install_command())
                .status()?;
            if !status.success() {
                anyhow::bail!("{}", installer_failed_message(lang, status));
            }
            println!("{}", updated_message(lang, &latest));
        }
        Some(latest) => {
            println!("{}", up_to_date_message(lang, &latest));
        }
        None => {
            println!("{}", check_failed_message(lang));
        }
    }

    Ok(())
}

fn cleanup_message(lang: &str, moved: usize, removed: usize) -> String {
    match lang {
        "ru" => format!("Очищено {moved} микро-тем и удалено {removed} устаревших тем"),
        _ => format!("Cleaned up {moved} micro-topics and removed {removed} stale topics"),
    }
}

fn dedupe_message(lang: &str, removed_items: usize, removed_topics: usize) -> String {
    match lang {
        "ru" => format!(
            "Удалено {removed_items} дубликатов учебных элементов и {removed_topics} дубликатов тем"
        ),
        _ => format!(
            "Removed {removed_items} duplicate learning items and {removed_topics} duplicate topics"
        ),
    }
}

fn updating_message(lang: &str, latest: &str) -> String {
    match lang {
        "ru" => format!("Обновление v{} → v{latest}...", update::CURRENT_VERSION),
        _ => format!("Updating v{} → v{latest}...", update::CURRENT_VERSION),
    }
}

fn installer_failed_message(lang: &str, status: std::process::ExitStatus) -> String {
    match lang {
        "ru" => format!("Установщик завершился с ошибкой: {status}"),
        _ => format!("Installer failed with status {status}"),
    }
}

fn updated_message(lang: &str, latest: &str) -> String {
    match lang {
        "ru" => {
            format!("Обновлено до v{latest}. Запустите `opencourse`, чтобы начать новую версию.")
        }
        _ => format!("Updated to v{latest}. Run `opencourse` to start the new version."),
    }
}

fn up_to_date_message(lang: &str, latest: &str) -> String {
    match lang {
        "ru" => format!(
            "Уже последняя версия (v{}, последний релиз v{latest}).",
            update::CURRENT_VERSION
        ),
        _ => format!(
            "Already up to date (v{}, latest release v{latest}).",
            update::CURRENT_VERSION
        ),
    }
}

fn check_failed_message(lang: &str) -> String {
    match lang {
        "ru" => format!(
            "Не удалось проверить обновления (сеть недоступна?). Текущая версия: v{}.",
            update::CURRENT_VERSION
        ),
        _ => format!(
            "Could not check for updates (network unavailable?). Current version: v{}.",
            update::CURRENT_VERSION
        ),
    }
}

/// One-off startup maintenance: removes fuzzy-duplicate learning items and
/// curriculum topics left over from earlier versions. Idempotent — does
/// nothing when there are no duplicates.
async fn dedupe_tables(db: &Database) -> anyhow::Result<(usize, usize)> {
    let items = db.learning_items().read_all().await?;
    let (_, removed_items) = open_course_cli::db::learning_items::dedupe(items);
    for item in &removed_items {
        db.learning_items().delete_by_id(&item.id).await?;
    }

    let curriculum = db.curriculum().read_all().await?;
    let (_, removed_topics) = open_course_cli::db::curriculum::dedupe(curriculum.topics);
    for topic in &removed_topics {
        db.curriculum().delete_by_topic_id(&topic.id).await?;
        let _ = db.progress().delete_by_topic_id(&topic.id).await;
        let _ = db.reviews().remove_by_topic_id(&topic.id).await;
    }

    Ok((removed_items.len(), removed_topics.len()))
}
