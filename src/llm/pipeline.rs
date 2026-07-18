//! Facade over the LLM pipeline modules, keeping the original import paths
//! (`crate::llm::pipeline::*`) stable for callers outside `llm`.

pub use crate::llm::analysis::{finalize_analysis_with_new_topics, generate_topic_metadata};
pub use crate::llm::curriculum::generate_curriculum;
pub use crate::llm::debug_log::log_debug_event;
pub use crate::llm::retry::{generate_analysis, generate_exercises};
pub use crate::llm::topic_review::{generate_topic_review, is_valid_topic_review};
