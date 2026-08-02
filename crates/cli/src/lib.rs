//! Binary crate plus re-exports used by the integration tests.
//!
//! The LLM prompt builders and response parsers live in
//! `open_course_core::llm` (moved there so the sync server can reuse them
//! via a git dependency); `open_course_llm` re-exports them.

pub mod app;
pub mod ui;
pub mod update;

pub use open_course_config as config;
pub use open_course_core as core;
pub use open_course_db as db;
pub use open_course_llm as llm;
