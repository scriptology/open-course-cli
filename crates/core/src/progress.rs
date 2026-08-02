#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ProgressTopic {
    pub topic_id: String,
    pub score: f64,
    pub mastery: f64,
    pub difficulty_estimate: f64,
    pub practice_count: i32,
    pub last_practiced: Option<String>,
    /// RFC3339 timestamp of the last modification; `None` means "unknown"
    /// (predates sync support) and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

impl ProgressTopic {
    /// A fresh, never-practiced progress entry starting at `initial_score`.
    pub fn initial(topic_id: String, initial_score: f64) -> Self {
        Self {
            topic_id,
            score: initial_score,
            mastery: initial_score,
            ..Default::default()
        }
    }
}

/// Starting score for a newly added topic: material below the user's CEFR
/// level is treated as already familiar (100), everything else starts at 0.
pub fn initial_topic_score(topic_cefr: i32, user_cefr: i32) -> f64 {
    if topic_cefr > 0 && topic_cefr < user_cefr {
        100.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProgressData {
    pub version: i32,
    pub topics: Vec<ProgressTopic>,
    pub session_count: i32,
    pub adaptive_alerts: Vec<String>,
}
