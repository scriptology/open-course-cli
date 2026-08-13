//! Data reset operations for the settings screen.

use open_course_core::error::Result;
use open_course_db::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetAction {
    Progress,
    History,
    Curriculum,
    Reviews,
    All,
}

impl ResetAction {
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

pub async fn execute_reset(db: &Database, action: ResetAction) -> Result<()> {
    use open_course_db::outbox::{
        ENTITY_LEARNING_ITEM, ENTITY_PROGRESS, ENTITY_SESSION, ENTITY_TOPIC, OP_TOMBSTONE_RESET,
    };

    let reset_at = chrono::Utc::now().to_rfc3339();
    let payload = crate::reset_payload(&reset_at);
    match action {
        ResetAction::Progress => {
            db.progress().reset().await?;
            crate::outbox_append(db, OP_TOMBSTONE_RESET, ENTITY_PROGRESS, "*", &payload).await;
        }
        ResetAction::History => {
            db.history().reset().await?;
            crate::outbox_append(db, OP_TOMBSTONE_RESET, ENTITY_SESSION, "*", &payload).await;
        }
        ResetAction::Curriculum => {
            db.curriculum().reset().await?;
            crate::outbox_append(db, OP_TOMBSTONE_RESET, ENTITY_TOPIC, "*", &payload).await;
        }
        ResetAction::Reviews => {
            // Reviews are not a synced entity.
            db.reviews().reset().await?;
        }
        ResetAction::All => {
            db.progress().reset().await?;
            db.history().reset().await?;
            db.curriculum().reset().await?;
            db.reviews().reset().await?;
            db.learning_items().reset().await?;
            db.lemmas().reset().await?;
            db.forms().reset().await?;
            // TODO: also emit tombstoneReset for lemma/form once old clients
            // are updated — their sync engine wipes all data on a tombstone
            // for an unknown entity.
            for entity in [
                ENTITY_PROGRESS,
                ENTITY_SESSION,
                ENTITY_TOPIC,
                ENTITY_LEARNING_ITEM,
            ] {
                crate::outbox_append(db, OP_TOMBSTONE_RESET, entity, "*", &payload).await;
            }
        }
    }
    let _ = db
        .metadata()
        .set(open_course_db::metadata::KEY_RESET_AT, &reset_at)
        .await;
    Ok(())
}
