//! Exec-context task-locals — let exec-class tools surface per-call context to
//! the sandbox without threading it through every trait signature.
//!
//! - [`SESSION_ID`] — which session is executing. Set by
//!   `invoke_with_session_trace`; read by `WorkspaceSandbox` to pick/lazy-create
//!   the per-session workspace.
//! - [`EXEC_JUSTIFICATION`] — the model's natural-language reason for *why* a
//!   capability escalation is needed. Set by `CodeExecTool` around the
//!   `Sandbox::execute` call when the LLM passes a `justification`; read by
//!   `WorkspaceSandbox` when it formats the human approval prompt, so the
//!   approver sees WHY (not just WHAT capabilities are requested). codex
//!   `justification` / hermes `force`-with-reason parity. R8/R9: the model
//!   explains in natural language, the system merely relays — zero added
//!   judgment.

use tokio::task_local;

use crate::session::service::SessionId;

task_local! {
    pub static SESSION_ID: SessionId;
    /// Model-supplied reason for the current escalating exec call. Only scoped
    /// when the LLM actually provided one — absence ⇒ the approval prompt stays
    /// byte-identical to its pre-justification form.
    pub static EXEC_JUSTIFICATION: String;
}

/// Returns the current session id if we're inside a `SESSION_ID.scope(...)`,
/// otherwise `None`. Outside a session scope, tools must fall back to a
/// shared "no-session" workspace (policy owned by `WorkspaceSandbox`).
#[must_use]
pub fn current_session() -> Option<SessionId> {
    SESSION_ID.try_with(|id| id.clone()).ok()
}

/// Returns the model-supplied justification for the current exec call, if one
/// was scoped via [`EXEC_JUSTIFICATION`]. `None` outside the scope (the common
/// case — most calls don't escalate and pass no justification).
#[must_use]
pub fn current_justification() -> Option<String> {
    EXEC_JUSTIFICATION.try_with(|j| j.clone()).ok()
}
