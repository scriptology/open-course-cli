pub mod migration;
pub mod profile;
pub mod provider;

pub use crate::provider::{ProviderConfig, ProviderId};

use serde::{Deserialize, Serialize};

use crate::profile::{LanguagePair, UserPreferences, UserProfile};
use open_course_core::error::{AppError, Result};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncConfig>,
}

/// Cloud sync settings. The access token is never stored here — it lives in
/// the `TokenStore` (OS keychain or a 0600 file).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

pub const DEFAULT_SYNC_SERVER_URL: &str = "https://api.open-course.eu";
pub const SYNC_SERVER_URL_ENV: &str = "OPEN_COURSE_SYNC_URL";

/// Sync server base URL: the `OPEN_COURSE_SYNC_URL` env var wins over the
/// config value, which wins over the production default.
pub fn resolve_sync_server_url(config: Option<&OpenCourseConfig>) -> String {
    if let Ok(url) = std::env::var(SYNC_SERVER_URL_ENV)
        && !url.is_empty()
    {
        return url;
    }
    config
        .and_then(|c| c.sync.as_ref())
        .and_then(|s| s.server_url.clone())
        .unwrap_or_else(|| DEFAULT_SYNC_SERVER_URL.to_string())
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
            sync: None,
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

/// Merges pairs known to the server (created on the web or another device)
/// into the local config. Returns the ids of the pairs that were added, in
/// server order. Existing pairs are left untouched.
pub fn merge_remote_pairs(
    config: &mut OpenCourseConfig,
    remote: &[open_course_core::sync_protocol::PairInfoResponse],
) -> Vec<String> {
    let mut added = Vec::new();
    for pair in remote {
        if config.pairs.iter().any(|p| p.id == pair.pair_id) {
            continue;
        }
        config.pairs.push(LanguagePair {
            id: pair.pair_id.clone(),
            profile: UserProfile {
                native_language: pair.native_lang.clone(),
                target_language: pair.target_lang.clone(),
                age: pair.age.and_then(|a| u32::try_from(a).ok()),
                self_assessed_cefr: pair.self_assessed_cefr.clone(),
            },
        });
        added.push(pair.pair_id.clone());
    }
    added
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

/// Default data directory: `~/.open-course-cli`, so the same data is used no
/// matter where the binary is launched from.
///
/// If the global directory has no config yet but the current directory has a
/// local `.open-course-cli/config.json` (pre-global-layout install), the local
/// directory is moved to the global location. Falls back to the current
/// directory when the home directory cannot be determined.
pub fn resolve_data_dir(cwd: &std::path::Path) -> std::path::PathBuf {
    resolve_data_dir_with_home(cwd, dirs::home_dir().as_deref())
}

pub fn resolve_data_dir_with_home(
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
) -> std::path::PathBuf {
    let Some(home) = home else {
        return cwd.to_path_buf();
    };
    if cwd == home {
        return home.to_path_buf();
    }
    let global = open_course_dir(home);
    if !has_config(home) && has_config(cwd) {
        migrate_local_data(&open_course_dir(cwd), &global);
    }
    home.to_path_buf()
}

/// Move entries from a legacy local data dir into the global one. Never
/// overwrites existing global entries; removes the local dir if it ends up
/// empty.
fn migrate_local_data(local: &std::path::Path, global: &std::path::Path) {
    if std::fs::create_dir_all(global).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(local) else {
        return;
    };
    for entry in entries.flatten() {
        let target = global.join(entry.file_name());
        if target.exists() {
            continue;
        }
        let _ = std::fs::rename(entry.path(), &target);
    }
    if std::fs::read_dir(local).is_ok_and(|mut e| e.next().is_none()) {
        let _ = std::fs::remove_dir(local);
        eprintln!(
            "Migrated data from {} to {}",
            local.display(),
            global.display()
        );
    } else {
        eprintln!(
            "Partially migrated data from {} to {}; some entries were left behind",
            local.display(),
            global.display()
        );
    }
}

pub fn pair_db_path(cwd: &std::path::Path, pair_id: &str) -> std::path::PathBuf {
    open_course_dir(cwd).join("pairs").join(pair_id).join("db")
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

#[cfg(test)]
mod tests {
    use super::*;
    use open_course_core::sync_protocol::PairInfoResponse;

    fn remote_pair(pair_id: &str, native: &str, target: &str) -> PairInfoResponse {
        PairInfoResponse {
            pair_id: pair_id.to_string(),
            native_lang: native.to_string(),
            target_lang: target.to_string(),
            revision: 1,
            age: Some(30),
            self_assessed_cefr: Some("B1".to_string()),
            batch_size: 3,
            topic_count: 0,
        }
    }

    fn test_config() -> OpenCourseConfig {
        OpenCourseConfig::new(
            ProviderId::OpenAi,
            ProviderConfig::ApiKey {
                api_key: None,
                model: "gpt-4o-mini".to_string(),
                base_url: None,
                endpoint: None,
                reasoning_effort: None,
                enable_thinking: None,
            },
            UserProfile {
                native_language: "en".to_string(),
                target_language: "de".to_string(),
                age: None,
                self_assessed_cefr: None,
            },
        )
    }

    #[test]
    fn merge_remote_pairs_adds_only_unknown_pairs() {
        let mut config = test_config();
        let remote = vec![
            remote_pair("en-de", "en", "de"),
            remote_pair("en-fr", "en", "fr"),
            remote_pair("en-es", "en", "es"),
        ];
        let added = merge_remote_pairs(&mut config, &remote);
        assert_eq!(added, vec!["en-fr".to_string(), "en-es".to_string()]);
        assert_eq!(config.pairs.len(), 3);
        let fr = config.find_pair("en-fr").unwrap();
        assert_eq!(fr.profile.native_language, "en");
        assert_eq!(fr.profile.target_language, "fr");
        assert_eq!(fr.profile.age, Some(30));
        assert_eq!(fr.profile.self_assessed_cefr.as_deref(), Some("B1"));
        // The pre-existing pair is untouched.
        assert_eq!(config.pairs[0].id, "en-de");
        assert_eq!(config.active_pair, "en-de");
    }

    #[test]
    fn merge_remote_pairs_is_idempotent() {
        let mut config = test_config();
        let remote = vec![remote_pair("en-fr", "en", "fr")];
        assert_eq!(merge_remote_pairs(&mut config, &remote).len(), 1);
        assert!(merge_remote_pairs(&mut config, &remote).is_empty());
        assert_eq!(config.pairs.len(), 2);
    }
}
