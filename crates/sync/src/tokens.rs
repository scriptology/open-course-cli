//! Access-token storage: the OS keychain when available, a `0600` file in
//! the data directory otherwise (headless Linux). The account name is
//! fixed ("default") because the user id is only known after the first
//! login; a new login simply overwrites the entry.

use std::path::{Path, PathBuf};

use crate::error::SyncError;
use crate::protocol::TokenSet;

const KEYRING_SERVICE: &str = "opencourse";
const KEYRING_ACCOUNT: &str = "default";
const AUTH_FILE: &str = "auth.json";

/// Which storage backend is in use — the UI warns when tokens are only in
/// a plain file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBackend {
    Keychain,
    File,
}

pub struct TokenStore {
    data_dir: PathBuf,
    backend: TokenBackend,
}

impl TokenStore {
    /// Picks the keychain when it answers, the file backend otherwise.
    pub fn new(data_dir: PathBuf) -> Self {
        let backend = match keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
            Ok(entry) => match entry.get_password() {
                // NoEntry means the keychain works, it just holds nothing.
                Ok(_) | Err(keyring::Error::NoEntry) => TokenBackend::Keychain,
                Err(_) => TokenBackend::File,
            },
            Err(_) => TokenBackend::File,
        };
        Self { data_dir, backend }
    }

    /// Forces a backend — used by tests (never touching the real keychain)
    /// and by callers that want a deterministic choice.
    pub fn with_backend(data_dir: PathBuf, backend: TokenBackend) -> Self {
        Self { data_dir, backend }
    }

    pub fn backend(&self) -> TokenBackend {
        self.backend
    }

    pub async fn save(&self, tokens: &TokenSet) -> Result<(), SyncError> {
        let json = serde_json::to_string(tokens)?;
        match self.backend {
            TokenBackend::Keychain => {
                let entry = keyring_entry()?;
                entry
                    .set_password(&json)
                    .map_err(|e| SyncError::TokenStore(e.to_string()))
            }
            TokenBackend::File => {
                let path = self.auth_file();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, json)?;
                set_owner_only_permissions(&path)?;
                Ok(())
            }
        }
    }

    pub async fn load(&self) -> Result<Option<TokenSet>, SyncError> {
        match self.backend {
            TokenBackend::Keychain => {
                let entry = keyring_entry()?;
                match entry.get_password() {
                    Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(SyncError::TokenStore(e.to_string())),
                }
            }
            TokenBackend::File => match std::fs::read_to_string(self.auth_file()) {
                Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
        }
    }

    pub async fn delete(&self) -> Result<(), SyncError> {
        match self.backend {
            TokenBackend::Keychain => {
                let entry = keyring_entry()?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(SyncError::TokenStore(e.to_string())),
                }
            }
            TokenBackend::File => match std::fs::remove_file(self.auth_file()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    fn auth_file(&self) -> PathBuf {
        self.data_dir.join(AUTH_FILE)
    }
}

fn keyring_entry() -> Result<keyring::Entry, SyncError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| SyncError::TokenStore(e.to_string()))
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
