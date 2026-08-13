//! Wire protocol types for cloud sync (camelCase JSON, additive changes
//! only). Shared between the sync client crate and the future server, which
//! can depend on `open-course-core` alone.
//!
//! Chosen device-poll style: the HTTP status carries the meaning and the
//! `error` field is informative. Authorized → 200 with the token JSON;
//! pending → 428 (`{"error": "authorization_pending"}`); expired → 400
//! (`{"error": "expired_token"}`). The client also accepts the bare
//! error-field style (any status) for forward compatibility.

use serde::{Deserialize, Serialize};

/// `POST {base}/auth/device` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// `POST {base}/auth/device/poll` success body (HTTP 200).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
}

/// Error body used by device poll (and accepted elsewhere).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ErrorBody {
    pub error: String,
}

/// A single change entry. `op` is one of "upsert" | "delete" |
/// "tombstoneReset"; `entity` is one of "topic" | "progress" | "session" |
/// "learningItem" | "lemma" | "form" | "metadata".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub seq: i64,
    pub op: String,
    pub entity: String,
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `POST {base}/v1/sync/push` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub pair_id: String,
    pub base_revision: i64,
    pub changes: Vec<Change>,
    /// When set, the server must replace its canonical curriculum with the
    /// pushed topics instead of merging (used by the "keep mine" conflict
    /// resolution) and return the new revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_curriculum: Option<bool>,
}

/// `POST {base}/v1/sync/push` success body (HTTP 2xx).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    pub revision: i64,
}

/// The server's canonical curriculum, returned on a push conflict (409) or
/// requested when binding a device to a pair that already has one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CurriculumPayload {
    pub revision: i64,
    /// Canonical curriculum version; bumped every time the canon is
    /// replaced. Defaults to 0 for older payloads.
    #[serde(default)]
    pub version: i32,
    #[serde(default)]
    pub topics: Vec<crate::curriculum::Topic>,
}

/// Push conflict body (HTTP 409): `{ "canonical": CurriculumPayload }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ConflictBody {
    pub canonical: CurriculumPayload,
}

/// `GET {base}/v1/sync/pull?pairId=..&since=..` response. `resetAt` is
/// present when the server wiped the pair's data (all clients must reset
/// before applying `changes`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PullResponse {
    pub revision: i64,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}

/// `GET {base}/v1/me` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub email: String,
    pub subscription_status: String,
}

/// Outbox op strings (snake_case) to wire strings (camelCase).
pub fn op_to_wire(op: &str) -> &str {
    match op {
        "tombstone_reset" => "tombstoneReset",
        other => other,
    }
}

/// Outbox entity strings (snake_case) to wire strings (camelCase).
pub fn entity_to_wire(entity: &str) -> &str {
    match entity {
        "learning_item" => "learningItem",
        other => other,
    }
}

/// Whether a wire `op` means "upsert" (accepts both naming styles).
pub fn op_is_upsert(op: &str) -> bool {
    op == "upsert"
}

/// Whether a wire `op` means "delete".
pub fn op_is_delete(op: &str) -> bool {
    op == "delete"
}

/// Whether a wire `op` means "tombstone reset" (accepts both naming styles).
pub fn op_is_tombstone_reset(op: &str) -> bool {
    op == "tombstoneReset" || op == "tombstone_reset"
}

/// Whether a wire `entity` names learning items (accepts both styles).
pub fn entity_is_learning_item(entity: &str) -> bool {
    entity == "learningItem" || entity == "learning_item"
}

/// Whether a wire `entity` names vocabulary lemmas (single-word entity,
/// passes through `entity_to_wire` unchanged).
pub fn entity_is_lemma(entity: &str) -> bool {
    entity == "lemma"
}

/// Whether a wire `entity` names vocabulary forms (single-word entity,
/// passes through `entity_to_wire` unchanged).
pub fn entity_is_form(entity: &str) -> bool {
    entity == "form"
}
