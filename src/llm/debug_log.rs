use std::path::Path;

use chrono::Utc;

use crate::error::Result;

pub(crate) fn log_raw_response(prompt: &str, raw: &str, kind: &str, data_dir: Option<&Path>) {
    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_none() {
        return;
    }

    let Some(data_dir) = data_dir else {
        return;
    };

    let _ = log_debug_text(
        kind,
        &format!("=== PROMPT ===\n{prompt}\n\n=== RAW RESPONSE ===\n{raw}\n"),
        data_dir,
    );
}

/// Always writes a failure dump and returns the file path. Used even when
/// OPEN_COURSE_CLI_DEBUG is off so users can inspect why parsing failed.
pub(crate) fn log_failed_response(
    prompt: &str,
    raw: &str,
    cleaned: &str,
    parse_errors: &str,
    kind: &str,
    data_dir: Option<&Path>,
) -> Option<String> {
    let data_dir = data_dir?;
    let text = format!(
        "=== PROMPT ===\n{prompt}\n\n=== RAW RESPONSE ({raw_len} chars) ===\n{raw}\n\n=== CLEANED JSON ===\n{cleaned}\n\n=== PARSE ERRORS ===\n{parse_errors}\n",
        raw_len = raw.len(),
    );
    log_debug_text(kind, &text, data_dir)
        .ok()
        .map(|path| path.to_string_lossy().to_string())
}

pub fn log_debug_event(kind: &str, message: &str, data_dir: Option<&Path>) {
    if std::env::var_os("OPEN_COURSE_CLI_DEBUG").is_none() {
        return;
    }
    let Some(data_dir) = data_dir else {
        return;
    };
    let _ = log_debug_text(kind, message, data_dir);
}

fn log_debug_text(kind: &str, text: &str, data_dir: &Path) -> Result<std::path::PathBuf> {
    let debug_dir = data_dir.join(".open-course-cli").join("debug");
    std::fs::create_dir_all(&debug_dir)?;

    let file_path = debug_dir.join(format!("{kind}-{}.txt", Utc::now().timestamp_millis()));
    std::fs::write(&file_path, text)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&file_path)
            .map(|m| m.permissions())
            .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o600));
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&file_path, perms);
    }

    let _ = cleanup_old_debug_files(&debug_dir, 20);
    Ok(file_path)
}

fn cleanup_old_debug_files(debug_dir: &Path, keep: usize) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(debug_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .collect();
    if entries.len() <= keep {
        return Ok(());
    }
    entries.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    entries.reverse();
    for entry in entries.iter().skip(keep) {
        let _ = std::fs::remove_file(entry.path());
    }
    Ok(())
}
