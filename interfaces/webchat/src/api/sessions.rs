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
