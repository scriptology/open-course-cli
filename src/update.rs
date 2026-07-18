use crate::error::{AppError, Result};

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const GITHUB_LATEST_API: &str =
    "https://api.github.com/repos/scriptology/open-course-cli/releases/latest";

const INSTALL_COMMAND: &str = "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/scriptology/open-course-cli/releases/latest/download/open-course-cli-installer.sh | sh";

/// Query GitHub for the latest release tag and return it without the leading
/// `v`. Any network or parse error is treated as "no update available" so the
/// app can always start offline.
pub async fn latest_release_version() -> Result<Option<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::Config(format!("Failed to build HTTP client: {e}")))?;

    let response = match client.get(GITHUB_LATEST_API).send().await {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let tag = match body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
    {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(None),
    };

    Ok(Some(tag))
}

/// Compare two dotted version strings (e.g. "0.1.0" vs "0.2.0"). The latest
/// string may include a leading `v`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    let current = current.trim_start_matches('v');
    let latest = latest.trim_start_matches('v');

    match (parse_version(current), parse_version(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    }
}

fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major = parts[0].parse().ok()?;
    let minor = parts[1].parse().ok()?;
    let patch = parts[2].parse().ok()?;
    Some((major, minor, patch))
}

/// Shell command the user can run to install the latest release.
pub fn install_command() -> &'static str {
    INSTALL_COMMAND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_when_major_increases() {
        assert!(is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn newer_when_minor_increases() {
        assert!(is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn newer_when_patch_increases() {
        assert!(is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn not_newer_when_equal() {
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn not_newer_when_older() {
        assert!(!is_newer("0.2.0", "0.1.0"));
    }

    #[test]
    fn strips_leading_v() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("v0.1.0", "0.2.0"));
    }

    #[test]
    fn invalid_version_is_not_newer() {
        assert!(!is_newer("0.1.0", "not-a-version"));
        assert!(!is_newer("not-a-version", "0.2.0"));
    }
}
