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

/// Persist (or clear) the reasoning depth bound to a session.
///
/// Fourth twin of [`set_exec_tier`]: `Some(level_id)` stores the level under
/// `SessionIdentityMeta.custom["think_level"]`, `None` writes a JSON null.
///
/// `None` does NOT mean "shallow" and does not mean "follow a global" — core
/// resolves depth as request > session > **no directive at all**, so clearing
/// the override leaves the provider on its own default. Surfaces must say that
/// rather than inventing a global to follow.
pub async fn set_think_level(
    state: &DashboardState,
    session_key: &str,
    level: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "metadata": { "think_level": level },
    });
    state.rpc_call("sessions.patch", params).await?;
    Ok(())
}

/// Persist (or clear) whether this conversation gets memory injected.
///
/// Fifth twin of [`set_exec_tier`]: `Some("on" | "off")` stores the mode under
/// `SessionIdentityMeta.custom["memory_mode"]`, `None` writes a JSON null and
/// the session follows the install-wide `[memory] enabled`.
///
/// `off` suppresses the three INJECTED envelopes (curated memory, the wiki
/// orientation index, per-query recall). It does not disable the memory tools
/// and it does not stop the conversation from recording what it learns — a
/// surface that describes it as "memory off" without that distinction is
/// promising something core deliberately does not do.
pub async fn set_memory_mode(
    state: &DashboardState,
    session_key: &str,
    mode: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "session_key": session_key,
        "metadata": { "memory_mode": mode },
    });
    state.rpc_call("sessions.patch", params).await?;
    Ok(())
}

/// The per-session dials a conversation carries.
///
/// Defined in `shared_ui_logic` — the rules for which of them ride a send live
/// beside it (`session_dials_for_send`), and both halves have to agree across
/// the wide composer, the phone composer and three session pickers.
pub use shared_ui_logic::state::SessionKnobs;

/// One row of `sessions.list`, as every Panel surface reads it.
///
/// Mirrors the server's `SessionInfo`. There were two hand-written copies of
/// this shape — the wide sidebar's and the phone history's — and they had
/// already diverged: the phone one carried no dials at all, so tapping a row
/// there restored the folder and dropped every knob. One decoder, one place to
/// add a field.
///
/// Unknown fields are ignored (serde's default), which is what makes an older
/// Panel survive a newer core; every field is `#[serde(default)]` so a newer
/// Panel survives an older core.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct SessionRow {
    pub key: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub message_count: u32,
    /// Unix epoch seconds; `None` sorts last.
    #[serde(default)]
    pub updated_at: Option<i64>,
    /// The session's project working directory. `None` ⇒ the default
    /// `~/.aleph/workspaces/{agent_id}` workspace.
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub exec_tier: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub think_level: Option<String>,
    #[serde(default)]
    pub memory_mode: Option<String>,
    #[serde(default)]
    pub model_pin: Option<String>,
}

impl SessionRow {
    /// The dials this row carries, as one value to hand to
    /// [`crate::views::chat::state::ChatState::apply_session_knobs`].
    #[must_use]
    pub fn knobs(&self) -> SessionKnobs {
        SessionKnobs {
            exec_tier: self.exec_tier.clone(),
            mode: self.mode.clone(),
            think_level: self.think_level.clone(),
            memory_mode: self.memory_mode.clone(),
            model_pin: self.model_pin.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field names are the wire contract with `SessionInfo`, and a typo
    /// here does not fail — it decodes to `None` and reads as "this session has
    /// no override", which is indistinguishable from the truth.
    #[test]
    fn a_row_decodes_every_dial_the_server_sends() {
        let row: SessionRow = serde_json::from_value(serde_json::json!({
            "key": "agent:main:main:s1",
            "agent_id": "main",
            "exec_tier": "ask",
            "mode": "code",
            "think_level": "high",
            "memory_mode": "off",
            "model_pin": "claude-opus-5",
            "project_root": "/tmp/p",
            "updated_at": 1_700_000_000_i64,
            // A field this Panel does not model must not fail the row.
            "compaction_count": 3,
        }))
        .expect("a full server row must decode");

        assert_eq!(
            row.knobs(),
            SessionKnobs {
                exec_tier: Some("ask".into()),
                mode: Some("code".into()),
                think_level: Some("high".into()),
                memory_mode: Some("off".into()),
                model_pin: Some("claude-opus-5".into()),
            }
        );
    }

    /// A session with no overrides, and an older core that sends none of these
    /// keys at all, must both land on "follow the default" — not on a panic and
    /// not on a value this client invented.
    #[test]
    fn a_bare_row_follows_the_defaults() {
        let row: SessionRow =
            serde_json::from_value(serde_json::json!({ "key": "agent:main:main:s1" }))
                .expect("a minimal row must decode");
        assert_eq!(row.knobs(), SessionKnobs::default());
        assert!(row.knobs().exec_tier.is_none());
    }
}
