//! Verifies `SESSION_ID` task-local is set at every supported tool entry point.
//!
//! The `with_session_scope` helper is the single source of truth for scoping
//! `SESSION_ID` around asynchronous tool work. Both `invoke_with_session_trace`
//! (Phase 2 trace wrapper) and direct tool-dispatch call sites (cron, heartbeat,
//! etc.) must funnel through this helper so exec-class tools such as
//! `CodeExecTool` can always discover the active session via
//! `sandbox::context::current_session()`.

use alephcore::routing::session_key::SessionKey;
use alephcore::session::tool_trace::with_session_scope;
use alephcore::session::SESSION_ID;

#[tokio::test]
async fn with_session_scope_sets_task_local() {
    let sid = SessionKey::ephemeral("test-session-42");
    let observed =
        with_session_scope(&sid, async { SESSION_ID.try_with(|s| s.clone()).ok() }).await;
    assert_eq!(observed, Some(sid));
}

#[tokio::test]
async fn outside_scope_no_session_id() {
    let observed = SESSION_ID.try_with(|s| s.clone()).ok();
    assert!(observed.is_none());
}

#[tokio::test]
async fn with_session_scope_visible_via_current_session_helper() {
    // Parity check: the sandbox-side convenience wrapper and the direct
    // `task_local` read must agree inside the same scope.
    let sid = SessionKey::ephemeral("parity-check");
    let (direct, via_helper) = with_session_scope(&sid, async {
        let direct = SESSION_ID.try_with(|s| s.clone()).ok();
        let via_helper = alephcore::sandbox::current_session();
        (direct, via_helper)
    })
    .await;
    assert_eq!(direct, Some(sid.clone()));
    assert_eq!(via_helper, Some(sid));
}

#[tokio::test]
async fn with_session_scope_restores_absence_after_exit() {
    let sid = SessionKey::ephemeral("scope-cleanup");
    let _ = with_session_scope(&sid, async { /* noop */ }).await;
    // Outside the scope, the task-local must be unset again.
    assert!(SESSION_ID.try_with(|s| s.clone()).is_err());
}
