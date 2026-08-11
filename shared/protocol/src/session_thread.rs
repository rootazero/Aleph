//! Thread-continuity contract — the request a thin client sends to `agent.run`
//! and the settings snapshot `chat.history` hands back when it re-attaches.
//!
//! # Why these types live here
//!
//! Same argument as [`crate::workspace`], and the same defect found a second
//! time. `aleph-tui` deliberately **cannot** depend on `alephcore` (its
//! `Cargo.toml` says so in capitals), so the `agent.run` wire shape was written
//! twice: once as `AgentRunParams` in the handler, once as a
//! `serde_json::json!` literal in `interfaces/tui/src/tui/commands.rs`.
//!
//! They disagreed. The handler's `input` is required — no `#[serde(default)]`,
//! no alias — and the TUI sent `message` (the key `chat.send` takes). Every
//! message ever typed into `aleph-tui` came back `INVALID_PARAMS`, through the
//! one call site whose own doc comment promised "the request shape can never
//! drift across the four paths": the shape was wrong at that single point, so
//! all four paths were wrong identically, and nothing went red because the
//! literal had nobody to disagree with.
//!
//! Sharing one type makes a rename a **compile** error on the client and a
//! **red test** on the server ([`AgentRunRequest`]'s reconciliation test lives
//! in `alephcore`, the crate that depends on both sides — the only place an
//! assertion can compare the two rather than compare a literal to itself).
//!
//! # What a thread carries
//!
//! [`SessionSnapshot`] is the other half. A conversation's durable settings —
//! usage mode, exec tier, thinking depth, model pin, cumulative tokens, memory
//! mode — have always been persisted server-side (`identity_meta.custom` and
//! the `sessions` row). No surface handed them to a client that re-attached by
//! key, so a client that reopened a thread showed the *global* defaults while
//! the server kept enforcing the *session's* values. The snapshot rides on
//! `chat.history` for the reason that response's own doc gives for carrying
//! `active_run` and `plan`: they are one snapshot, and any second call opens a
//! window where a client holds the transcript but not the settings that govern
//! it.

use serde::{Deserialize, Serialize};

/// Parameters for `agent.run` as a thin client sends them.
///
/// A deliberate **subset** of the server's `AgentRunParams`: exactly the fields
/// a terminal / CLI client can supply. Panel-only fields (attachments,
/// `model_override`, `project_root`, `peer_id`) are absent because no client
/// that shares this type sends them, and a field here with no sender is the
/// same defect in the other direction.
///
/// Every field but `input` is skipped when absent, so the emitted object is
/// byte-identical to the hand-written literal it replaces for the common case.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunRequest {
    /// The user's message. **Required** — this is the field the TUI got wrong.
    pub input: String,

    /// Canonical session key (`agent:{id}:main:s{n}`, …). Omit to let the
    /// gateway route one.
    ///
    /// Note what happens when this is present but unparseable: `AgentRouter::route`
    /// falls through to its own routing and mints a **new epoch**, silently. A
    /// client that invents its own key format therefore gets a fresh
    /// conversation on every launch and addresses a row no other RPC can find.
    /// Clients must echo back what the server returned (`session_key` in the
    /// `agent.run` result), never a key of their own construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,

    /// Channel identifier (`"cli:term1"`, `"gui:window1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    /// Explicit target agent, bypassing channel-binding resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Reasoning depth for this turn (`"off"`…`"xhigh"` and the documented
    /// aliases). A value here is also stamped onto the session, so it outlives
    /// the turn that carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,

    /// Usage mode for this turn (`"chat"` / `"work"` / `"code"`). Same
    /// stamped-onto-the-session contract as `thinking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Execution tier for this turn (`"ask"` / `"auto"` / `"full"`). Carried on
    /// the message because a brand-new conversation has no session row to write
    /// it to yet — the first turn is the one a tier is most needed for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
}

impl AgentRunRequest {
    /// A run request carrying only text, on an existing session.
    #[must_use]
    pub fn new(session_key: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            session_key: Some(session_key.into()),
            ..Self::default()
        }
    }
}

/// The result of `agent.run` / `chat.send`, as a thin client reads it.
///
/// `session_key` is the field that matters for thread continuity: it is the
/// **canonical** key the gateway routed to, which is not necessarily the one
/// the client asked for. A client that keeps its own key instead of adopting
/// this one is addressing a session that may not exist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunAccepted {
    /// Id of the accepted run.
    #[serde(default)]
    pub run_id: String,
    /// Canonical session key this run was routed to.
    #[serde(default)]
    pub session_key: String,
    /// RFC 3339 acceptance timestamp.
    #[serde(default)]
    pub accepted_at: String,
}

/// A conversation's durable settings, as `chat.history` reports them on
/// re-attach.
///
/// Every field is read back from something the server already persists — this
/// type creates no storage, it connects storage that had no reader:
///
/// | field | where it lives |
/// |---|---|
/// | `mode` / `exec_tier` / `think_level` / `memory_mode` / `model_pin` | `sessions.metadata` → `identity_meta.custom[…]` |
/// | `model` / `model_provider` | `sessions.model` / `sessions.model_provider`, written per run by `session_projector` |
/// | `input_tokens` / `output_tokens` / `total_tokens` / `estimated_cost_usd` | the `sessions` row's usage columns |
/// | `project_root` | `identity_meta.custom["project_root"]` |
///
/// `None` on a knob means **follow the global default**, not "off" — the same
/// three-valued contract the pickers use. A client must render an unset knob as
/// "following global", never as a concrete value it picked itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Canonical session key this snapshot describes.
    pub session_key: String,

    /// Agent this conversation is bound to.
    #[serde(default)]
    pub agent_id: String,

    /// Per-session usage mode override (`"chat"` / `"work"` / `"code"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Per-session execution tier override (`"ask"` / `"auto"` / `"full"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,

    /// Per-session reasoning depth override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,

    /// Per-session memory mode (`"on"` / `"off"`). `None` follows `[memory] enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,

    /// The model the conversation is **pinned** to (`select_model`), if any.
    ///
    /// Distinct from [`Self::model`]: a pin is a forward-looking instruction,
    /// while `model` records what last served. They disagree for exactly one
    /// turn after a pick — the pick applies from the next run — and a client
    /// that shows only `model` reports the model the user just switched away
    /// from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,

    /// Provider pinned alongside [`Self::model_pin`], if the pick named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin_provider: Option<String>,

    /// The model that last served a run on this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// The provider that last served a run on this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Cumulative prompt tokens billed to this conversation.
    #[serde(default)]
    pub input_tokens: i64,

    /// Cumulative completion tokens billed to this conversation.
    #[serde(default)]
    pub output_tokens: i64,

    /// `input_tokens + output_tokens`, as the session row records it.
    #[serde(default)]
    pub total_tokens: i64,

    /// Priced cost of everything billed to this conversation, in USD.
    #[serde(default)]
    pub estimated_cost_usd: f64,

    /// Messages recorded on this conversation.
    #[serde(default)]
    pub message_count: i64,

    /// How many times this conversation has been compacted.
    #[serde(default)]
    pub compaction_count: i64,

    /// User-chosen working directory, if the conversation was scoped to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,

    /// User-facing label, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl SessionSnapshot {
    /// The model a client should *display* for this conversation: the pin when
    /// one is armed, else whatever last served.
    ///
    /// One derivation, because the two facts disagree for a turn and every
    /// surface that re-derives the rule will eventually pick a different one.
    #[must_use]
    pub fn effective_model(&self) -> Option<&str> {
        self.model_pin
            .as_deref()
            .or(self.model.as_deref())
            .filter(|m| !m.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_run_request_emits_input_not_message() {
        let req = AgentRunRequest::new("agent:main:main", "hello");
        let v = serde_json::to_value(&req).expect("serialize");
        assert_eq!(v["input"], "hello");
        assert_eq!(v["session_key"], "agent:main:main");
        // The bug this type exists to make impossible.
        assert!(
            v.get("message").is_none(),
            "`message` is `chat.send`'s key; `agent.run` takes `input`"
        );
    }

    #[test]
    fn absent_options_are_not_emitted() {
        let req = AgentRunRequest::new("k", "hi");
        let v = serde_json::to_value(&req).expect("serialize");
        let obj = v.as_object().expect("object");
        // Byte-for-byte the shape the hand-written literal produced, minus the
        // wrong key: adding optional fields must not change what an existing
        // client sends.
        assert_eq!(obj.len(), 2, "unexpected keys: {obj:?}");
    }

    #[test]
    fn effective_model_prefers_the_pin() {
        let mut snap = SessionSnapshot {
            model: Some("gpt-5".into()),
            ..SessionSnapshot::default()
        };
        assert_eq!(snap.effective_model(), Some("gpt-5"));
        snap.model_pin = Some("claude-opus-5".into());
        assert_eq!(
            snap.effective_model(),
            Some("claude-opus-5"),
            "a pick the user just made must win over the model it replaced"
        );
    }

    #[test]
    fn effective_model_ignores_an_empty_string() {
        // `sessions.model` is written from a run's report; an empty string
        // there is absence, not a model named "".
        let snap = SessionSnapshot {
            model: Some(String::new()),
            ..SessionSnapshot::default()
        };
        assert_eq!(snap.effective_model(), None);
    }

    #[test]
    fn an_unset_knob_round_trips_as_absent() {
        let snap = SessionSnapshot {
            session_key: "agent:main:main".into(),
            ..SessionSnapshot::default()
        };
        let v = serde_json::to_value(&snap).expect("serialize");
        for knob in ["mode", "exec_tier", "think_level", "memory_mode"] {
            assert!(
                v.get(knob).is_none(),
                "{knob} must be absent, not null: absent means \"follow global\""
            );
        }
        let back: SessionSnapshot = serde_json::from_value(v).expect("round trip");
        assert_eq!(back, snap);
    }
}
