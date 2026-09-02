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

/// One row of `sessions.list`, as every Panel surface reads it — the type the
/// server constructs the row from, not a copy of its field names.
///
/// This was a hand-written mirror of the server's row, and before that two
/// mirrors: the wide sidebar's and the phone history's, already diverged (the
/// phone one carried no dials at all, so tapping a row there restored the
/// folder and dropped every knob). Collapsing them to one Panel-side decoder
/// fixed that half and left the other: a subset reader can only ever prove it
/// is a superset of whatever happens to arrive, so a field renamed on the
/// server degrades here to a `#[serde(default)]` — a session with no override
/// where there is one, or a run badge that never appears (criterion #10).
///
/// A field a client wants is now added to [`SessionListRow`] itself, in
/// `shared/protocol`, which makes it a compile-time fact on the server too.
pub use aleph_protocol::SessionListRow;

/// The dials a `sessions.list` row carries, as one value to hand to
/// [`crate::views::chat::state::ChatState::apply_session_knobs`].
///
/// An extension trait because [`SessionListRow`] is defined in
/// `shared/protocol`, which has no business knowing about this crate's
/// `SessionKnobs` — the wire shape belongs to the contract, the reduction to
/// the surface that renders it.
pub trait SessionRowKnobs {
    /// The five per-session overrides this row reports.
    fn knobs(&self) -> SessionKnobs;
}

impl SessionRowKnobs for SessionListRow {
    fn knobs(&self) -> SessionKnobs {
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

    /// The dials reach the surface, built from the shared row rather than from
    /// a JSON literal written here — a literal would only prove serde
    /// round-trips its own bytes, which stays true through a server-side
    /// rename.
    #[test]
    fn a_row_hands_over_every_dial_the_server_sent() {
        let row = SessionListRow {
            key: "agent:main:main:s1".into(),
            agent_id: "main".into(),
            exec_tier: Some("ask".into()),
            mode: Some("code".into()),
            think_level: Some("high".into()),
            memory_mode: Some("off".into()),
            model_pin: Some("claude-opus-5".into()),
            project_root: Some("/tmp/p".into()),
            updated_at: 1_700_000_000,
            ..SessionListRow::default()
        };
        let wire = serde_json::to_value(&row).expect("the row serialises");
        let parsed: SessionListRow = serde_json::from_value(wire).expect("and parses back");

        assert_eq!(
            parsed.knobs(),
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
        let row: SessionListRow =
            serde_json::from_value(serde_json::json!({ "key": "agent:main:main:s1" }))
                .expect("a minimal row must decode");
        assert_eq!(row.knobs(), SessionKnobs::default());
        assert!(row.knobs().exec_tier.is_none());
    }

    /// A field this Panel does not model must not cost the whole row: a list
    /// row that fails to parse renders as a session that does not exist.
    #[test]
    fn an_unmodelled_field_does_not_fail_the_row() {
        let row: SessionListRow = serde_json::from_value(serde_json::json!({
            "key": "agent:main:main:s1",
            "a_field_from_a_newer_core": 3,
        }))
        .expect("unknown keys are ignored");
        assert_eq!(row.key, "agent:main:main:s1");
    }
}
