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
    /// Module the session belonged to, when it was a unit session.
    #[serde(default)]
    pub module_id: Option<String>,
    /// Module units the session practiced (ids in the `unit_` namespace).
    #[serde(default)]
    pub target_unit_ids: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_without_module_fields_deserializes() {
        // Summaries written before situational modules have no
        // module_id/target_unit_ids.
        let json = r#"{
            "id": "s1",
            "date": "2025-01-01T00:00:00Z",
            "target_topic_ids": ["t1"],
            "side_topic_ids": [],
            "new_topic_ids": [],
            "avg_target_score": 80.0,
            "target_delta": 5.0
        }"#;
        let summary: SessionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.module_id, None);
        assert_eq!(summary.target_unit_ids, None);
        assert_eq!(summary.target_topic_ids, ["t1"]);
    }

    #[test]
    fn summary_module_fields_round_trip() {
        let summary = SessionSummary {
            module_id: Some("mod_1".to_string()),
            target_unit_ids: Some(vec!["unit_1".to_string()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&summary).unwrap();
        let parsed: SessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, summary);
    }
}
