//! Access-token storage: a `0600` file in the data directory
//! (`~/.open-course-cli/auth.json`). The OS keychain was dropped on purpose:
//! unsigned/self-updated binaries trigger scary keychain prompts on macOS,
//! and a silent fallback made the CLI lose track of a perfectly valid login.
//! A file with owner-only permissions is the standard practice for CLI tools
//! (`~/.aws/credentials`, `~/.config/gh/hosts.yml`) and behaves the same
//! everywhere. The account name is fixed ("default") because the user id is
//! only known after the first login; a new login simply overwrites the file.

use std::path::{Path, PathBuf};

use crate::error::SyncError;
use crate::protocol::TokenSet;

const AUTH_FILE: &str = "auth.json";

pub struct TokenStore {
    data_dir: PathBuf,
}

impl TokenStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub async fn save(&self, tokens: &TokenSet) -> Result<(), SyncError> {
        let json = serde_json::to_string(tokens)?;
        let path = self.auth_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, json)?;
        set_owner_only_permissions(&path)?;
        Ok(())
    }

    pub async fn load(&self) -> Result<Option<TokenSet>, SyncError> {
        match std::fs::read_to_string(self.auth_file()) {
            Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn delete(&self) -> Result<(), SyncError> {
        match std::fs::remove_file(self.auth_file()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn auth_file(&self) -> PathBuf {
        self.data_dir.join(AUTH_FILE)
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<(), SyncError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<(), SyncError> {
    Ok(())
}
