//! LLM prompt builders and response parsing, shared with external consumers
//! (e.g. the server crate) that drive the same model contract.

pub mod parse;
pub mod prompts;
pub mod response;
