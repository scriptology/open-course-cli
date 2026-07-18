use crate::app::AppState;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetAction {
    Progress,
    History,
    Curriculum,
    Reviews,
    All,
}

impl ResetAction {
    pub fn label(&self) -> &'static str {
        match self {
            ResetAction::Progress => "reset progress",
            ResetAction::History => "reset history",
            ResetAction::Curriculum => "reset curriculum",
            ResetAction::Reviews => "reset reviews",
            ResetAction::All => "reset all data",
        }
    }

    /// Title of the field row in the Data section.
    pub fn field_label(&self) -> &'static str {
        match self {
            ResetAction::Progress => "Reset progress",
            ResetAction::History => "Reset history",
            ResetAction::Curriculum => "Reset curriculum",
            ResetAction::Reviews => "Reset reviews",
            ResetAction::All => "Reset all",
        }
    }

    /// Description shown as the field value in the Data section.
    pub fn description(&self) -> &'static str {
        match self {
            ResetAction::Progress => "Clear all progress scores",
            ResetAction::History => "Clear all session history",
            ResetAction::Curriculum => "Clear all curriculum topics",
            ResetAction::Reviews => "Clear all topic reviews",
            ResetAction::All => "Clear all data",
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
