pub mod migration;
pub mod profile;
pub mod provider;

pub use crate::config::provider::{ProviderConfig, ProviderId};

use serde::{Deserialize, Serialize};

use crate::config::profile::{UserPreferences, UserProfile};
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenCourseConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    pub active_provider: ProviderId,
    pub providers: std::collections::HashMap<ProviderId, ProviderConfig>,
    pub profile: UserProfile,
    #[serde(default)]
    pub preferences: UserPreferences,
}

impl OpenCourseConfig {
    pub fn new(
        provider_id: ProviderId,
        provider_config: ProviderConfig,
        profile: UserProfile,
    ) -> Self {
        let mut providers = std::collections::HashMap::new();
        providers.insert(provider_id, provider_config);
        Self {
            version: default_version(),
            active_provider: provider_id,
            providers,
            profile,
            preferences: UserPreferences::default(),
        }
    }
}

fn default_version() -> u32 {
    1
}

pub fn read_config(cwd: &std::path::Path) -> Result<Option<OpenCourseConfig>> {
    let path = open_course_dir(cwd).join("config.json");
    if !path.exists() {
        // Try legacy migration from profile.md.
        if let Some((config, legacy_path)) = migration::try_migrate_from_profile_md(cwd)? {
            write_config(&config, cwd)?;
            // Only rename the legacy file after the new config has been
            // successfully written, so a crash leaves the original intact.
            let backup_path = legacy_path.with_extension("md.backup");
            std::fs::rename(&legacy_path, backup_path)?;
            return Ok(Some(config));
        }
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let mut config: OpenCourseConfig = serde_json::from_str(&content)?;
    migration::migrate_legacy_config(&mut config);
    Ok(Some(config))
}

pub fn write_config(config: &OpenCourseConfig, cwd: &std::path::Path) -> Result<()> {
    let dir = open_course_dir(cwd);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let temp = dir.join("config.json.tmp");
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub fn has_config(cwd: &std::path::Path) -> bool {
    open_course_dir(cwd).join("config.json").exists()
}

pub fn open_course_dir(cwd: &std::path::Path) -> std::path::PathBuf {
    cwd.join(".open-course-cli")
}

pub fn ensure_open_course_gitignore(cwd: &std::path::Path) -> Result<()> {
    let dir = open_course_dir(cwd);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(".gitignore");
    if !path.exists() {
        std::fs::write(&path, "*\n")?;
    }
    Ok(())
}
