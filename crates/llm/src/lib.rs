pub mod analysis;
pub mod client;
pub mod curriculum;
pub mod debug_log;
pub mod diagnostics;
pub mod factory;
pub mod model_listing;
pub mod pipeline;
pub mod provider;
pub mod result;
pub mod retry;
pub mod streaming;
pub mod topic_review;
pub mod transport;

// Prompt builders and response parsing live in `open-course-core` so external
// consumers (e.g. the server) can reuse them without the LLM client stack.
// Re-exported here to keep the existing `open_course_llm::{prompts, parse}`
// paths working.
pub use open_course_core::llm::parse;
pub use open_course_core::llm::prompts;

pub use result::LlmResult;

#[cfg(test)]
pub(crate) mod env_test_lock {
    pub(crate) static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
