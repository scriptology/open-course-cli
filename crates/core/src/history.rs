pub const MAX_HISTORY_ENTRIES: usize = 500;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SessionSummary {
    pub id: String,
    pub date: String,
    pub target_topic_ids: Vec<String>,
    pub side_topic_ids: Vec<String>,
    pub new_topic_ids: Vec<String>,
    pub avg_target_score: f64,
    pub target_delta: f64,
    /// RFC3339 timestamp of when the summary was written; `None` means
    /// "unknown" (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
}
