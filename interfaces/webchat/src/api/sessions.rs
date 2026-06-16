//! Session management RPC wrappers (Panel → Core).
use crate::context::DashboardState;

/// Pin or unpin a session. Persists server-side (cross-device, R6).
pub async fn set_pinned(
    state: &DashboardState,
    session_key: &str,
    pinned: bool,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "pinned": pinned,
    });
    state.rpc_call("sessions.set_pinned", params).await?;
    Ok(())
}

/// Persist (or clear) the project working directory bound to a session.
/// `Some(path)` stores it; `None` reverts to the default agent workspace.
/// Persists server-side (cross-device, R6) so the Panel can restore the active
/// project after a reload. Stamped automatically on the first message of a
/// project run; this wrapper lets the Panel set/clear it explicitly too.
pub async fn set_project_root(
    state: &DashboardState,
    session_key: &str,
    project_root: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "project_root": project_root,
    });
    state.rpc_call("sessions.set_project_root", params).await?;
    Ok(())
}
