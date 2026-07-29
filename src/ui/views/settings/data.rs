use crate::app::AppState;
use crate::error::Result;
use crate::ui::labels::SettingsLabels;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetAction {
    Progress,
    History,
    Curriculum,
    Reviews,
    All,
}

impl ResetAction {
    pub fn label(&self, labels: &SettingsLabels) -> &'static str {
        match self {
            ResetAction::Progress => labels.reset_progress_action,
            ResetAction::History => labels.reset_history_action,
            ResetAction::Curriculum => labels.reset_curriculum_action,
            ResetAction::Reviews => labels.reset_reviews_action,
            ResetAction::All => labels.reset_all_action,
        }
    }

    /// Title of the field row in the Data section.
    pub fn field_label(&self, labels: &SettingsLabels) -> &'static str {
        match self {
            ResetAction::Progress => labels.reset_progress_title,
            ResetAction::History => labels.reset_history_title,
            ResetAction::Curriculum => labels.reset_curriculum_title,
            ResetAction::Reviews => labels.reset_reviews_title,
            ResetAction::All => labels.reset_all_title,
        }
    }

    /// Description shown as the field value in the Data section.
    pub fn description(&self, labels: &SettingsLabels) -> &'static str {
        match self {
            ResetAction::Progress => labels.reset_progress_desc,
            ResetAction::History => labels.reset_history_desc,
            ResetAction::Curriculum => labels.reset_curriculum_desc,
            ResetAction::Reviews => labels.reset_reviews_desc,
            ResetAction::All => labels.reset_all_desc,
        }
    }

    pub fn all() -> &'static [ResetAction] {
        &[
            ResetAction::Progress,
            ResetAction::History,
            ResetAction::Curriculum,
            ResetAction::Reviews,
            ResetAction::All,
        ]
    }

    pub fn from_field(field: usize) -> Option<Self> {
        Self::all().get(field).copied()
    }
}

pub async fn execute_reset(state: &mut AppState, action: ResetAction) -> Result<()> {
    let db = state.db.clone();
    match action {
        ResetAction::Progress => {
            db.progress().reset().await?;
        }
        ResetAction::History => {
            db.history().reset().await?;
        }
        ResetAction::Curriculum => {
            db.curriculum().reset().await?;
        }
        ResetAction::Reviews => {
            db.reviews().reset().await?;
        }
        ResetAction::All => {
            db.progress().reset().await?;
            db.history().reset().await?;
            db.curriculum().reset().await?;
            db.reviews().reset().await?;
        }
    }
    Ok(())
}
