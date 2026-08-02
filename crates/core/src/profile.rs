use serde::{Deserialize, Serialize};

/// Language-learner profile shared with the prompts and the server.
/// Field names and serde attributes are part of the config file and wire
/// format — change them only with a migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    pub native_language: String,
    pub target_language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_assessed_cefr: Option<String>,
}
