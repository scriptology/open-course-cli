pub mod migration;
pub mod profile;
pub mod provider;

pub use crate::config::provider::{ProviderConfig, ProviderId};

use serde::{Deserialize, Serialize};

use crate::config::profile::{LanguagePair, UserPreferences, UserProfile};
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenCourseConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    pub active_provider: ProviderId,
    pub providers: std::collections::HashMap<ProviderId, ProviderConfig>,
    #[serde(default)]
    pub preferences: UserPreferences,
    #[serde(default)]
    pub pairs: Vec<LanguagePair>,
    #[serde(default)]
    pub active_pair: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<UserProfile>,
}

impl OpenCourseConfig {
    pub fn new(
        provider_id: ProviderId,
        provider_config: ProviderConfig,
        profile: UserProfile,
    ) -> Self {
        let mut providers = std::collections::HashMap::new();
        providers.insert(provider_id, provider_config);
        let pair = LanguagePair {
            id: Self::pair_id(&profile.native_language, &profile.target_language),
            profile,
        };
        let active_pair = pair.id.clone();
        Self {
            version: default_version(),
            active_provider: provider_id,
            providers,
            preferences: UserPreferences::default(),
            pairs: vec![pair],
            active_pair,
            profile: None,
        }
    }

    pub fn pair_id(native: &str, target: &str) -> String {
        format!("{}-{}", native.to_lowercase(), target.to_lowercase())
    }

    pub fn active_profile(&self) -> &UserProfile {
        self.pairs
            .iter()
            .find(|p| p.id == self.active_pair)
            .map(|p| &p.profile)
            .unwrap_or_else(|| {
                self.pairs
                    .first()
                    .map(|p| &p.profile)
                    .expect("config has at least one pair")
            })
    }

    pub fn active_profile_mut(&mut self) -> &mut UserProfile {
        let active_id = self.active_pair.clone();
        if let Some(pos) = self.pairs.iter().position(|p| p.id == active_id) {
            return &mut self.pairs[pos].profile;
        }
        &mut self
            .pairs
            .first_mut()
            .expect("config has at least one pair")
            .profile
    }

    pub fn add_pair(&mut self, profile: UserProfile) -> Result<&str> {
        let id = Self::pair_id(&profile.native_language, &profile.target_language);
        if self.pairs.iter().any(|p| p.id == id) {
            return Err(AppError::Config(format!(
                "Language pair {} already exists",
                id
            )));
        }
        self.pairs.push(LanguagePair {
            id: id.clone(),
            profile,
        });
        Ok(self.pairs.last().map(|p| p.id.as_str()).unwrap())
    }

    pub fn find_pair(&self, id: &str) -> Option<&LanguagePair> {
        self.pairs.iter().find(|p| p.id == id)
    }
}

fn default_version() -> u32 {
    2
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
    if migration::migrate_legacy_config(cwd, &mut config)? {
        write_config(&config, cwd)?;
    }
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

pub fn pair_db_path(cwd: &std::path::Path, pair_id: &str) -> std::path::PathBuf {
    open_course_dir(cwd)
        .join("pairs")
        .join(pair_id)
        .join("db")
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
