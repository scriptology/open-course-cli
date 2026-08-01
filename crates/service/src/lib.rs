//! Service layer: business logic shared between the CLI (a thin adapter) and
//! any future frontends (e.g. an HTTP server). UI state lives in the CLI;
//! this crate only talks to the database, the domain, and the LLM.

pub mod curriculum;
pub mod reset;
pub mod session;

use open_course_db::Database;

/// Best-effort outbox append: sync bookkeeping must never fail the local
/// operation it follows.
pub(crate) async fn outbox_append(
    db: &Database,
    op: &str,
    entity: &str,
    entity_id: &str,
    payload: &str,
) {
    let _ = db.outbox().append(op, entity, entity_id, payload).await;
}

/// Payload of a `tombstone_reset` entry: the moment the local data was wiped.
pub(crate) fn reset_payload(reset_at: &str) -> String {
    format!("{{\"reset_at\":\"{reset_at}\"}}")
}
