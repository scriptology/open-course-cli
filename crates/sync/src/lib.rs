//! Cloud sync client for open-course.
//!
//! This crate defines the sync protocol the future server must implement:
//! device-flow authentication, outbox push with conflict handling, and
//! delta pull with last-writer-wins application. All protocol types are
//! camelCase JSON; only additive changes are allowed.

mod bind;
mod client;
mod engine;
pub mod error;
pub mod protocol;
mod tokens;

pub use bind::{BindScenario, ProgressMerge};
pub use client::{PollResult, SyncClient};
pub use error::{PushError, SyncError};
pub use protocol::{
    Change, CurriculumPayload, DeviceCodeResponse, MeResponse, PullResponse, PushRequest,
    PushResponse, TokenSet,
};
pub use tokens::{TokenBackend, TokenStore};
