pub mod analysis;
pub mod client;
pub mod curriculum;
pub mod debug_log;
pub mod diagnostics;
pub mod factory;
pub mod model_listing;
pub mod parse;
pub mod pipeline;
pub mod prompts;
pub mod provider;
pub mod result;
pub mod retry;
pub mod streaming;
pub mod topic_review;
pub mod transport;

pub use result::LlmResult;

#[cfg(test)]
pub(crate) mod env_test_lock {
    pub(crate) static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
