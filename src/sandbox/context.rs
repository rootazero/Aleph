//! SESSION_ID task-local — lets exec-class tools discover which session is
//! currently executing without threading the id through every trait signature.
//!
//! Set by `invoke_with_session_trace` (session service side). Read by
//! `WorkspaceSandbox` to pick/lazy-create the per-session workspace.

use tokio::task_local;

use crate::session::service::SessionId;

task_local! {
    pub static SESSION_ID: SessionId;
}

/// Returns the current session id if we're inside a `SESSION_ID.scope(...)`,
/// otherwise `None`. Outside a session scope, tools must fall back to a
/// shared "no-session" workspace (policy owned by `WorkspaceSandbox`).
pub fn current_session() -> Option<SessionId> {
    SESSION_ID.try_with(|id| id.clone()).ok()
}
