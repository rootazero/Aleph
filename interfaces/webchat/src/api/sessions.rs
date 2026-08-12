//! Session management RPC wrappers (Panel → Core).
use crate::context::DashboardState;

/// Persist (or clear) the execution tier bound to a session.
///
/// `Some(tier_id)` stores the tier under `SessionIdentityMeta.custom["exec_tier"]`
/// — the same carrier as `custom["project_root"]` — where the run loop reads it
/// per turn; a session tier REPLACES the global tier for that session. `None`
/// writes a JSON null, which core reads as "absent" and falls back to the global
/// tier. Uses the existing `sessions.patch` RPC (its `metadata` object is merged
/// key-by-key into `custom`), so no new method is needed. Takes effect on the
/// next tool call.
pub async fn set_exec_tier(
    state: &DashboardState,
    session_key: &str,
    tier: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "metadata": { "exec_tier": tier },
    });
    state.rpc_call("sessions.patch", params).await?;
    Ok(())
}

/// Persist (or clear) the usage mode bound to a session.
///
/// Third twin of [`set_exec_tier`]: `Some(mode_id)` stores the mode under
/// `SessionIdentityMeta.custom["session_mode"]`, `None` writes a JSON null
/// (follow the global `[policies] mode`). Takes effect on the next turn.
pub async fn set_session_mode(
    state: &DashboardState,
    session_key: &str,
    mode: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "metadata": { "session_mode": mode },
    });
    state.rpc_call("sessions.patch", params).await?;
    Ok(())
}

/// Enter or leave the read-only planning phase for a session.
///
/// `true` stores `"planning"`; `false` writes a JSON null, which is how a user
/// leaves planning **without** approving a plan (the approval path clears it
/// server-side instead, from inside the run that asked). Takes effect on the
/// next turn — the floor a run is already under was fixed when that run started,
/// and only an approved handoff lifts it mid-run.
pub async fn set_plan_phase(
    state: &DashboardState,
    session_key: &str,
    planning: bool,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "metadata": { "plan_phase": if planning { Some("planning") } else { None } },
    });
    state.rpc_call("sessions.patch", params).await?;
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
