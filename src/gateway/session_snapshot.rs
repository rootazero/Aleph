//! One reading of "what settings is this conversation running under".
//!
//! A session's knobs are persisted in two places and nowhere else: the
//! `identity_meta.custom` bag (`exec_tier` / `session_mode` / `think_level` /
//! `memory_mode` / `model_pin` / `project_root`) and the `sessions` row's own
//! columns (`model`, `model_provider`, the usage counters). Both have been
//! durable for a long time. What was missing is a **reader**: no response
//! handed them to a client that re-attached to a conversation by key, so a
//! client that reopened a thread painted the *install* defaults over a
//! conversation the server was still governing by its *own* values.
//!
//! This module is that reader, and there is deliberately only one of it. Two
//! surfaces need the answer — `chat.history` (attach) and `sessions.list`
//! (browse) — and a knob decoded two ways is a knob whose two surfaces can
//! disagree about whether it is set. `sessions.list` used to decode three of
//! them inline; it now goes through here, which is also how `think_level` (the
//! twin that was simply never added) reached that surface at all.
//!
//! Every getter returns `None` for *both* "absent" and "JSON null", because
//! that is what the writers mean: `sessions.patch` clears an override by
//! writing `null`, and the run loop treats absent and null identically. A
//! client must render `None` as "follows the global default" — never as a
//! concrete value of its own choosing, which is the mistake that made a
//! restarted terminal claim a tier the session did not have.

use aleph_protocol::SessionSnapshot;

use crate::gateway::session_store::types::SessionMetadata;

/// `identity_meta.custom` key holding a session's user-chosen working
/// directory. Written by `sessions.set_project_root` and by
/// `resume_coordinator`; both spell it as a literal, so this constant is the
/// name the *readers* agree on.
pub const PROJECT_ROOT_SESSION_KEY: &str = "project_root";

/// Read one `identity_meta.custom` string, treating JSON `null` as absent.
///
/// The single decoder for every knob. Inlining `im.custom.get(k)?.as_str()` at
/// each call site is how `sessions.list` came to decode three knobs and
/// `chat.history` zero — the shape is trivial, which is exactly why it gets
/// copied instead of shared.
#[must_use]
pub fn custom_str(meta: &SessionMetadata, key: &str) -> Option<String> {
    meta.identity_meta
        .as_ref()?
        .custom
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Build the client-facing settings snapshot for a session.
///
/// Pure: it reads the metadata it is given and invents nothing. In particular
/// it does **not** fall back to the global tier/mode/think defaults — the
/// server resolves those per turn from live config, and a snapshot that baked
/// today's global value in would go stale the moment the config changed while
/// still looking authoritative.
#[must_use]
pub fn snapshot_from_metadata(meta: &SessionMetadata) -> SessionSnapshot {
    use crate::agents::thinking::THINK_LEVEL_SESSION_KEY;
    use crate::config::types::policies::{EXEC_TIER_SESSION_KEY, MODE_SESSION_KEY};
    use crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY;
    use crate::providers::session_model_handle::{
        MODEL_PIN_PROVIDER_SESSION_KEY, MODEL_PIN_SESSION_KEY,
    };

    SessionSnapshot {
        session_key: meta.key.clone(),
        agent_id: meta.agent_id.clone(),
        mode: custom_str(meta, MODE_SESSION_KEY),
        exec_tier: custom_str(meta, EXEC_TIER_SESSION_KEY),
        think_level: custom_str(meta, THINK_LEVEL_SESSION_KEY),
        memory_mode: custom_str(meta, MEMORY_MODE_SESSION_KEY),
        model_pin: custom_str(meta, MODEL_PIN_SESSION_KEY),
        model_pin_provider: custom_str(meta, MODEL_PIN_PROVIDER_SESSION_KEY),
        model: meta.model.clone(),
        model_provider: meta.model_provider.clone(),
        input_tokens: meta.input_tokens,
        output_tokens: meta.output_tokens,
        total_tokens: meta.total_tokens,
        estimated_cost_usd: meta.estimated_cost_usd,
        message_count: meta.message_count,
        compaction_count: meta.compaction_count,
        project_root: custom_str(meta, PROJECT_ROOT_SESSION_KEY),
        label: meta.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::SessionIdentityMeta;

    fn meta_with(custom: serde_json::Value) -> SessionMetadata {
        let mut identity = SessionIdentityMeta::default();
        if let serde_json::Value::Object(map) = custom {
            for (k, v) in map {
                identity.custom.insert(k, v);
            }
        }
        SessionMetadata {
            key: "agent:main:main".to_string(),
            agent_id: "main".to_string(),
            identity_meta: Some(identity),
            ..SessionMetadata::default()
        }
    }

    #[test]
    fn every_persisted_knob_reaches_the_snapshot() {
        let snap = snapshot_from_metadata(&meta_with(serde_json::json!({
            "session_mode": "code",
            "exec_tier": "ask",
            "think_level": "high",
            "memory_mode": "off",
            "model_pin": "claude-opus-5",
            "model_pin_provider": "anthropic",
            "project_root": "/tmp/proj",
        })));
        assert_eq!(snap.mode.as_deref(), Some("code"));
        assert_eq!(snap.exec_tier.as_deref(), Some("ask"));
        assert_eq!(snap.think_level.as_deref(), Some("high"));
        assert_eq!(snap.memory_mode.as_deref(), Some("off"));
        assert_eq!(snap.model_pin.as_deref(), Some("claude-opus-5"));
        assert_eq!(snap.model_pin_provider.as_deref(), Some("anthropic"));
        assert_eq!(snap.project_root.as_deref(), Some("/tmp/proj"));
        assert_eq!(snap.session_key, "agent:main:main");
        assert_eq!(snap.agent_id, "main");
    }

    /// `null` is how `sessions.patch` clears an override. Reading it as a value
    /// would make a cleared knob render as the string "null" in a pill.
    #[test]
    fn an_explicit_null_reads_as_no_override() {
        let snap = snapshot_from_metadata(&meta_with(serde_json::json!({
            "session_mode": serde_json::Value::Null,
            "exec_tier": serde_json::Value::Null,
            "think_level": serde_json::Value::Null,
        })));
        assert_eq!(snap.mode, None);
        assert_eq!(snap.exec_tier, None);
        assert_eq!(snap.think_level, None);
    }

    #[test]
    fn a_session_with_no_identity_meta_yields_no_overrides() {
        let meta = SessionMetadata {
            key: "agent:main:main".to_string(),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.mode, None);
        assert_eq!(snap.exec_tier, None);
        assert_eq!(snap.think_level, None);
        assert_eq!(snap.memory_mode, None);
        assert_eq!(snap.model_pin, None);
        assert_eq!(snap.project_root, None);
    }

    #[test]
    fn usage_columns_ride_verbatim() {
        let meta = SessionMetadata {
            key: "agent:main:main".to_string(),
            input_tokens: 1_200,
            output_tokens: 340,
            total_tokens: 1_540,
            estimated_cost_usd: 0.0421,
            message_count: 9,
            compaction_count: 2,
            model: Some("gpt-5".to_string()),
            model_provider: Some("openai".to_string()),
            ..SessionMetadata::default()
        };
        let snap = snapshot_from_metadata(&meta);
        assert_eq!(snap.input_tokens, 1_200);
        assert_eq!(snap.output_tokens, 340);
        assert_eq!(snap.total_tokens, 1_540);
        assert!((snap.estimated_cost_usd - 0.0421).abs() < f64::EPSILON);
        assert_eq!(snap.message_count, 9);
        assert_eq!(snap.compaction_count, 2);
        assert_eq!(snap.model.as_deref(), Some("gpt-5"));
        assert_eq!(snap.model_provider.as_deref(), Some("openai"));
    }

    /// Census, enforced against the source rather than a remembered list: every
    /// session-knob constant this crate defines must be read here.
    ///
    /// The defect this guards is the one that put `think_level` on no client
    /// surface at all for as long as it has existed — its two twins were added
    /// to `sessions.list`, it was not, and nothing noticed because a knob with
    /// no reader looks exactly like a knob nobody set.
    #[test]
    fn no_session_knob_constant_is_left_unread() {
        let src = include_str!("session_snapshot.rs");
        // Strip the test module so the constants named in *these* assertions do
        // not satisfy the check by themselves.
        let production = crate::utils::source_scan::production_prefix(src);
        for knob in [
            "MODE_SESSION_KEY",
            "EXEC_TIER_SESSION_KEY",
            "THINK_LEVEL_SESSION_KEY",
            "MEMORY_MODE_SESSION_KEY",
            "MODEL_PIN_SESSION_KEY",
            "PROJECT_ROOT_SESSION_KEY",
        ] {
            assert!(
                production.contains(knob),
                "{knob} is persisted on the session but never read into the snapshot — \
                 a client re-attaching cannot restore it"
            );
        }
    }
}
