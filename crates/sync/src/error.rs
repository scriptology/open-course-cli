//! Error types of the sync client.

use open_course_core::error::AppError;

use crate::protocol::CurriculumPayload;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("unauthorized: the access token was rejected; sign in again")]
    Unauthorized,

    #[error("server error: {0}")]
    Server(String),

    #[error("database error: {0}")]
    Db(#[from] AppError),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("token store error: {0}")]
    TokenStore(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for SyncError {
    fn from(e: serde_json::Error) -> Self {
        SyncError::Protocol(e.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error(transparent)]
    Sync(#[from] SyncError),

    /// The server rejected a curriculum change because its canonical
    /// curriculum has moved ahead. The local outbox is left intact; the
    /// caller should pull, reconcile, and push again.
    #[error("curriculum conflict: canonical revision {}", .0.revision)]
    CurriculumConflict(CurriculumPayload),
}
