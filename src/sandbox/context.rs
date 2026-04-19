//! Task-local SESSION_ID (Task 5 turns this into the real task_local).

use crate::session::service::SessionId;

/// Placeholder — replaced with tokio::task_local! in Task 5.
pub fn current_session() -> Option<SessionId> {
    None
}
