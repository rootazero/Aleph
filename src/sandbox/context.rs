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
//! - [`LIVE_TAIL`] — where the platform drivers' output drain loops tee a
//!   rolling tail of the child's stdout/stderr, so a *still-running* background
//!   job can be observed instead of being a black box until it exits.
//!
//! Every one of these is a `tokio::task_local`, which means: **the scope does
//! not cross `tokio::spawn`**. A tool that hands work to a detached task must
//! re-enter the scopes inside that task — see `bash_exec::spawn_background`,
//! which re-enters `SESSION_ID` and `LIVE_TAIL` for exactly this reason.

use std::sync::Arc;

use tokio::task_local;

use crate::sandbox::live_tail::LiveTail;
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

task_local! {
    pub static SESSION_ID: SessionId;
    /// Model-supplied reason for the current escalating exec call. Only scoped
    /// when the LLM actually provided one — absence ⇒ the approval prompt stays
    /// byte-identical to its pre-justification form.
    pub static EXEC_JUSTIFICATION: String;
    /// The command sandbox a worktree-isolated subagent's exec tools must use.
    /// Scoped around the subagent run by [`with_sandbox_override`] so `bash` /
    /// `code_exec` / `code_check` run at the worktree path (with
    /// `CARGO_TARGET_DIR` redirected) instead of the parent's shared workspace.
    /// Absent ⇒ tools use their construction-time sandbox (the common path).
    pub static SANDBOX_OVERRIDE: Arc<dyn Sandbox>;
    /// Rolling tail of the currently-executing child's output. Scoped by
    /// `bash`'s background spawner around the whole exec call; read by
    /// [`run_child_with_drain`](crate::sandbox::platforms::common::run_child_with_drain)
    /// which tees every chunk its drain loops read into it. Absent ⇒ nothing is
    /// tee'd and the drain loops behave byte-identically to their pre-live-tail
    /// form (the foreground path, which has no one to show a partial to).
    pub static LIVE_TAIL: Arc<LiveTail>;
}

/// Returns the current session id if we're inside a `SESSION_ID.scope(...)`,
/// otherwise `None`. Outside a session scope, tools must fall back to a
/// shared "no-session" workspace (policy owned by `WorkspaceSandbox`).
#[must_use]
pub fn current_session() -> Option<SessionId> {
    // rust-doctor-disable-next-line excessive-clone
    SESSION_ID.try_with(|id| id.clone()).ok()
}

/// Returns the model-supplied justification for the current exec call, if one
/// was scoped via [`EXEC_JUSTIFICATION`]. `None` outside the scope (the common
/// case — most calls don't escalate and pass no justification).
#[must_use]
pub fn current_justification() -> Option<String> {
    // rust-doctor-disable-next-line excessive-clone
    EXEC_JUSTIFICATION.try_with(|j| j.clone()).ok()
}

/// The exec-tool sandbox override in scope for the current call, if any. Command
/// tools prefer this over their construction-time sandbox so a worktree-isolated
/// subagent's commands run inside its checkout.
#[must_use]
pub fn current_sandbox_override() -> Option<Arc<dyn Sandbox>> {
    // rust-doctor-disable-next-line excessive-clone
    SANDBOX_OVERRIDE.try_with(|s| s.clone()).ok()
}

/// The live output tail in scope for the current exec call, if any. `None` on
/// the foreground path — nobody can read a partial from a call that has not
/// returned yet, so nothing is tee'd there.
#[must_use]
pub fn current_live_tail() -> Option<Arc<LiveTail>> {
    // rust-doctor-disable-next-line excessive-clone
    LIVE_TAIL.try_with(Arc::clone).ok()
}

/// Run `fut` with `sandbox` installed as the exec-tool sandbox override. `None`
/// runs `fut` unchanged (the common, non-isolated path). Mirrors
/// `tools::fs_scope::with_fs_scope`.
pub async fn with_sandbox_override<F>(sandbox: Option<Arc<dyn Sandbox>>, fut: F) -> F::Output
where
    F: std::future::Future,
{
    match sandbox {
        Some(s) => SANDBOX_OVERRIDE.scope(s, fut).await,
        None => fut.await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sandbox_override_absent_outside_scope() {
        assert!(current_sandbox_override().is_none());
    }

    #[tokio::test]
    async fn sandbox_override_visible_inside_and_cleared_after() {
        let sb: Arc<dyn Sandbox> = Arc::new(crate::sandbox::NoopSandbox);
        let seen =
            with_sandbox_override(Some(sb), async { current_sandbox_override().is_some() }).await;
        assert!(seen, "override must be visible inside the scope");
        assert!(
            current_sandbox_override().is_none(),
            "override must clear once the scope ends"
        );
    }

    #[tokio::test]
    async fn live_tail_visible_inside_and_cleared_after() {
        let tail = Arc::new(crate::sandbox::live_tail::LiveTail::new());
        let seen = LIVE_TAIL
            .scope(tail, async { current_live_tail().is_some() })
            .await;
        assert!(seen, "tail must be visible inside the scope");
        assert!(
            current_live_tail().is_none(),
            "tail must clear once the scope ends"
        );
    }

    /// The reason `bash_exec::spawn_background` re-enters the scope *inside*
    /// the detached task: task-locals do not cross `tokio::spawn`. If this ever
    /// starts passing with the scope only on the outside, the re-entry in
    /// `spawn_background` can be simplified — until then it is load-bearing.
    #[tokio::test]
    async fn live_tail_does_not_cross_tokio_spawn() {
        let tail = Arc::new(crate::sandbox::live_tail::LiveTail::new());
        let seen_inside_spawn = LIVE_TAIL
            .scope(tail, async {
                tokio::spawn(async { current_live_tail().is_some() })
                    .await
                    .expect("join")
            })
            .await;
        assert!(!seen_inside_spawn);
    }

    #[tokio::test]
    async fn sandbox_override_none_is_a_noop() {
        let seen =
            with_sandbox_override(None, async { current_sandbox_override().is_some() }).await;
        assert!(!seen);
    }
}
