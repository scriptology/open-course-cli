use std::io;

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Arrow/serialization failure in the storage layer (message kept as a
    /// string so this crate stays free of heavy storage dependencies).
    #[error("Arrow error: {0}")]
    Arrow(String),

    /// LanceDB failure in the storage layer (message kept as a string so
    /// this crate stays free of heavy storage dependencies).
    #[error("Database error: {0}")]
    Db(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Provider unavailable: {0}. Please try a different model or provider.")]
    ProviderUnavailable(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Invalid provider configuration: {0}")]
    ProviderConfig(String),

    #[error("Language code error: {0}")]
    LanguageCode(String),

    #[error("Not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
