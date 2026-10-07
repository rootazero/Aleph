//! Wire shapes for the `terminal` tool's `read` / `wait` / `explain` actions.
//!
//! Defined here, in the protocol crate, so the server builds its responses
//! from these types instead of hand-rolled `json!`. The agent state and entry
//! types are reused from [`crate::runtime`], never copied.

use crate::runtime::{RuntimeAgentEntry, RuntimeAgentState};
use serde::{Deserialize, Serialize};

/// Params for actions that address one session (`read`, `explain`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalSessionParams {
    pub session_id: String,
}

/// Params for `wait`. `until: None` (omitted) selects the tool's default set;
/// `Some(vec![])` is an explicit empty set and stays distinct from omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalWaitParams {
    pub session_id: String,
    #[serde(default)]
    pub until: Option<Vec<RuntimeAgentState>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalReadResponse {
    pub session_id: String,
    pub text: String,
}

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWaitOutcome {
    Reached,
    Timeout,
    Gone,
}

/// `agent` is always present on the wire (`null` when the table has no row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWaitResponse {
    pub session_id: String,
    pub outcome: TerminalWaitOutcome,
    pub agent: Option<RuntimeAgentEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalExplainRule {
    pub id: String,
    pub priority: i32,
    pub region: String,
    pub state: RuntimeAgentState,
}

/// What the detection engine was shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalExplainInputs {
    pub title: String,
    pub osc_progress: String,
    pub screen_tail: String,
}

/// Unknowns serialize as `null`; only `reason` is omitted when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TerminalExplainResponse {
    pub session_id: String,
    pub agent: Option<String>,
    pub state: RuntimeAgentState,
    pub matched_rule: Option<TerminalExplainRule>,
    pub source: Option<String>,
    pub manifest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub inputs: TerminalExplainInputs,
}

#[cfg(test)]
mod tests {
    //! Contract tests for the `terminal` tool's `read` / `wait` / `explain`
    //! wire shapes (A1). Expected keys and spellings are hand-written
    //! literals, never derived from the DTOs under test, so adding,
    //! renaming or dropping a field fails here instead of silently drifting
    //! from the JSON envelope callers already parse.

    use super::*;
    use crate::runtime::{RuntimeAgentEntry, RuntimeAgentState};
    use serde_json::{json, Value};
    use std::collections::BTreeSet;

    fn keys(v: &Value) -> BTreeSet<&str> {
        v.as_object()
            .expect("wire value is a JSON object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn set<const N: usize>(names: [&'static str; N]) -> BTreeSet<&'static str> {
        names.into_iter().collect()
    }

    fn entry() -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: "s1".into(),
            label: "claude".into(),
            cwd: "/tmp".into(),
            agent: Some("claude".into()),
            program: Some("claude".into()),
            state: RuntimeAgentState::Blocked,
            updated_at: 42,
            quiet_since: Some(41),
        }
    }

    // ---- session params / read ------------------------------------------

    /// Breaks if the addressing field is renamed or made optional: `read`
    /// and `explain` would then accept a request that names no session.
    #[test]
    fn session_params_require_session_id_and_carry_only_it() {
        let p: TerminalSessionParams = serde_json::from_value(json!({"session_id": "s1"})).unwrap();
        assert_eq!(
            p,
            TerminalSessionParams {
                session_id: "s1".into()
            }
        );
        assert_eq!(
            keys(&serde_json::to_value(&p).unwrap()),
            set(["session_id"])
        );
        assert!(serde_json::from_value::<TerminalSessionParams>(json!({})).is_err());
    }

    /// The `read` envelope is exactly `{session_id, text}`; a third key is
    /// over-sending on a surface the model reads every turn.
    #[test]
    fn read_response_wire_keys_and_roundtrip() {
        let r = TerminalReadResponse {
            session_id: "s1".into(),
            text: "$ ls\nfile".into(),
        };
        let wire = serde_json::to_value(&r).unwrap();
        assert_eq!(keys(&wire), set(["session_id", "text"]));
        assert_eq!(wire, json!({"session_id": "s1", "text": "$ ls\nfile"}));
        assert_eq!(
            serde_json::from_value::<TerminalReadResponse>(wire).unwrap(),
            r
        );
    }

    // ---- wait params -----------------------------------------------------

    /// Omitted `until` / `timeout_ms` must deserialize to `None` (the tool
    /// then applies its defaults); breaks if they become required.
    #[test]
    fn wait_params_omitted_optionals_default_to_none() {
        let p: TerminalWaitParams = serde_json::from_value(json!({"session_id": "s1"})).unwrap();
        assert_eq!(
            p,
            TerminalWaitParams {
                session_id: "s1".into(),
                until: None,
                timeout_ms: None,
            }
        );
    }

    /// `until: []` is a caller error the tool refuses ("requires at least
    /// one state"); folding it into "omitted" would silently apply the
    /// default set instead. The two must stay distinguishable.
    #[test]
    fn wait_params_explicit_empty_until_is_distinct_from_omitted() {
        let omitted: TerminalWaitParams =
            serde_json::from_value(json!({"session_id": "s1"})).unwrap();
        let empty: TerminalWaitParams =
            serde_json::from_value(json!({"session_id": "s1", "until": []})).unwrap();
        assert_eq!(omitted.until, None);
        assert_eq!(empty.until, Some(vec![]));
        assert_ne!(omitted, empty);
    }

    #[test]
    fn wait_params_parse_lowercase_states_and_timeout_from_literal_json() {
        let p: TerminalWaitParams = serde_json::from_str(
            r#"{"session_id":"s1","until":["idle","blocked","working","unknown"],"timeout_ms":1500}"#,
        )
        .unwrap();
        assert_eq!(
            p.until,
            Some(vec![
                RuntimeAgentState::Idle,
                RuntimeAgentState::Blocked,
                RuntimeAgentState::Working,
                RuntimeAgentState::Unknown,
            ])
        );
        assert_eq!(p.timeout_ms, Some(1500));
        assert_eq!(
            keys(&serde_json::to_value(&p).unwrap()),
            set(["session_id", "until", "timeout_ms"])
        );
        let back: TerminalWaitParams =
            serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn wait_params_roundtrip_preserves_omitted_and_empty() {
        for p in [
            TerminalWaitParams {
                session_id: "a".into(),
                until: None,
                timeout_ms: None,
            },
            TerminalWaitParams {
                session_id: "a".into(),
                until: Some(vec![]),
                timeout_ms: Some(0),
            },
        ] {
            let back: TerminalWaitParams =
                serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
            assert_eq!(back, p);
        }
    }

    #[test]
    fn wait_params_reject_an_unknown_state_word() {
        assert!(serde_json::from_value::<TerminalWaitParams>(
            json!({"session_id": "s1", "until": ["Idle"]})
        )
        .is_err());
        assert!(serde_json::from_value::<TerminalWaitParams>(
            json!({"session_id": "s1", "until": ["asleep"]})
        )
        .is_err());
    }

    // ---- wait outcome / response ----------------------------------------

    /// The three outcome spellings callers branch on. Breaks on any rename
    /// or a casing change (`Reached`, `timed_out`, ...).
    #[test]
    fn wait_outcome_spellings_are_reached_timeout_gone() {
        for (outcome, word) in [
            (TerminalWaitOutcome::Reached, "reached"),
            (TerminalWaitOutcome::Timeout, "timeout"),
            (TerminalWaitOutcome::Gone, "gone"),
        ] {
            assert_eq!(serde_json::to_value(outcome).unwrap(), json!(word));
            let back: TerminalWaitOutcome = serde_json::from_value(json!(word)).unwrap();
            assert_eq!(back, outcome);
        }
        assert!(serde_json::from_value::<TerminalWaitOutcome>(json!("Reached")).is_err());
        assert!(serde_json::from_value::<TerminalWaitOutcome>(json!("timed_out")).is_err());
    }

    /// `agent` stays present (as `null`) when there is no row — the
    /// existing envelope never omitted it — and, when present, is the
    /// protocol's own `RuntimeAgentEntry` shape rather than a copy.
    #[test]
    fn wait_response_keeps_agent_key_and_reuses_the_runtime_entry_shape() {
        let reached = TerminalWaitResponse {
            session_id: "s1".into(),
            outcome: TerminalWaitOutcome::Reached,
            agent: Some(entry()),
        };
        let wire = serde_json::to_value(&reached).unwrap();
        assert_eq!(keys(&wire), set(["session_id", "outcome", "agent"]));
        assert_eq!(wire["outcome"], "reached");
        assert_eq!(
            keys(&wire["agent"]),
            set([
                "session_id",
                "label",
                "cwd",
                "agent",
                "program",
                "state",
                "updated_at",
                "quiet_since"
            ])
        );
        assert_eq!(wire["agent"]["state"], "blocked");
        assert_eq!(
            serde_json::from_value::<TerminalWaitResponse>(wire).unwrap(),
            reached
        );

        let gone = TerminalWaitResponse {
            session_id: "s1".into(),
            outcome: TerminalWaitOutcome::Gone,
            agent: None,
        };
        let wire = serde_json::to_value(&gone).unwrap();
        assert_eq!(keys(&wire), set(["session_id", "outcome", "agent"]));
        assert!(wire["agent"].is_null());
        assert_eq!(wire["outcome"], "gone");
        assert_eq!(
            serde_json::from_value::<TerminalWaitResponse>(wire).unwrap(),
            gone
        );
    }

    /// Parses the envelope as callers see it today (hand-written JSON, not
    /// produced by the DTO) so an envelope change cannot hide behind a
    /// symmetric serialize/deserialize rename.
    #[test]
    fn wait_response_parses_the_existing_json_envelope() {
        let r: TerminalWaitResponse =
            serde_json::from_str(r#"{"session_id":"s9","outcome":"timeout","agent":null}"#)
                .unwrap();
        assert_eq!(r.session_id, "s9");
        assert_eq!(r.outcome, TerminalWaitOutcome::Timeout);
        assert_eq!(r.agent, None);
    }

    // ---- explain ----------------------------------------------------------

    fn inputs(tail: &str) -> TerminalExplainInputs {
        TerminalExplainInputs {
            title: "claude".into(),
            osc_progress: "".into(),
            screen_tail: tail.into(),
        }
    }

    #[test]
    fn explain_rule_wire_keys_and_lowercase_state() {
        let rule = TerminalExplainRule {
            id: "needs-approval".into(),
            priority: -3,
            region: "bottom".into(),
            state: RuntimeAgentState::Blocked,
        };
        let wire = serde_json::to_value(&rule).unwrap();
        assert_eq!(keys(&wire), set(["id", "priority", "region", "state"]));
        assert_eq!(
            wire,
            json!({"id": "needs-approval", "priority": -3, "region": "bottom", "state": "blocked"})
        );
        assert_eq!(
            serde_json::from_value::<TerminalExplainRule>(wire).unwrap(),
            rule
        );
    }

    #[test]
    fn explain_inputs_wire_keys() {
        let i = inputs("a\nb");
        let wire = serde_json::to_value(&i).unwrap();
        assert_eq!(keys(&wire), set(["title", "osc_progress", "screen_tail"]));
        assert_eq!(
            serde_json::from_value::<TerminalExplainInputs>(wire).unwrap(),
            i
        );
    }

    /// A matched rule: every key present, `reason` is omitted (absent, not
    /// `null`) because the engine had nothing to apologise for.
    #[test]
    fn explain_response_with_rule_omits_reason_and_keeps_other_nulls_out() {
        let r = TerminalExplainResponse {
            session_id: "s1".into(),
            agent: Some("claude".into()),
            state: RuntimeAgentState::Working,
            matched_rule: Some(TerminalExplainRule {
                id: "spinner".into(),
                priority: 10,
                region: "tail".into(),
                state: RuntimeAgentState::Working,
            }),
            source: Some("bundled".into()),
            manifest_version: Some("2026.1".into()),
            reason: None,
            inputs: inputs("esc to interrupt"),
        };
        let wire = serde_json::to_value(&r).unwrap();
        assert_eq!(
            keys(&wire),
            set([
                "session_id",
                "agent",
                "state",
                "matched_rule",
                "source",
                "manifest_version",
                "inputs"
            ])
        );
        assert!(
            wire.get("reason").is_none(),
            "None reason must be absent, not null"
        );
        assert_eq!(wire["state"], "working");
        assert_eq!(wire["matched_rule"]["state"], "working");
        assert_eq!(
            serde_json::from_value::<TerminalExplainResponse>(wire).unwrap(),
            r
        );
    }

    /// No agent: the unknowns stay as explicit `null`s (the existing
    /// envelope) and the reason is present. Distinguishes "we never looked"
    /// from "key left out".
    #[test]
    fn explain_response_without_agent_keeps_null_keys_and_carries_reason() {
        let r = TerminalExplainResponse {
            session_id: "s2".into(),
            agent: None,
            state: RuntimeAgentState::Unknown,
            matched_rule: None,
            source: None,
            manifest_version: None,
            reason: Some("no agent".into()),
            inputs: inputs(""),
        };
        let wire = serde_json::to_value(&r).unwrap();
        assert_eq!(
            keys(&wire),
            set([
                "session_id",
                "agent",
                "state",
                "matched_rule",
                "source",
                "manifest_version",
                "reason",
                "inputs"
            ])
        );
        for k in ["agent", "matched_rule", "source", "manifest_version"] {
            assert!(wire[k].is_null(), "{k} must serialize as null");
        }
        assert_eq!(wire["reason"], "no agent");
        assert_eq!(wire["state"], "unknown");
        assert_eq!(
            serde_json::from_value::<TerminalExplainResponse>(wire).unwrap(),
            r
        );
    }

    /// Hand-written JSON without `reason` still parses, to `None`.
    #[test]
    fn explain_response_parses_envelope_lacking_reason() {
        let r: TerminalExplainResponse = serde_json::from_str(
            r#"{"session_id":"s3","agent":null,"state":"idle","matched_rule":null,
                "source":null,"manifest_version":null,
                "inputs":{"title":"","osc_progress":"","screen_tail":""}}"#,
        )
        .unwrap();
        assert_eq!(r.reason, None);
        assert_eq!(r.state, RuntimeAgentState::Idle);
    }

    /// `screen_tail` is multi-line, non-ASCII text. It must cross the wire
    /// byte-for-byte: real newlines escaped (one JSON line), unicode kept,
    /// no normalisation of whitespace.
    #[test]
    fn explain_screen_tail_survives_real_newlines_and_unicode() {
        let tail = "line one\n日本語 ✓ é\n  indented  \n🙂 done";
        let r = TerminalExplainResponse {
            session_id: "s4".into(),
            agent: None,
            state: RuntimeAgentState::Unknown,
            matched_rule: None,
            source: None,
            manifest_version: None,
            reason: None,
            inputs: inputs(tail),
        };
        let text = serde_json::to_string(&r).unwrap();
        assert!(
            !text.contains('\n'),
            "a real newline must be escaped on the wire"
        );
        assert!(
            text.contains(r"line one\n日本語"),
            "escaped newline, literal unicode: {text}"
        );
        let wire: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(wire["inputs"]["screen_tail"], tail);
        let back: TerminalExplainResponse = serde_json::from_str(&text).unwrap();
        assert_eq!(back.inputs.screen_tail, tail);
        assert_eq!(back, r);

        // A peer that escapes the unicode must decode to the same text.
        let escaped = r#"{"title":"","osc_progress":"","screen_tail":"a\n\u65e5"}"#;
        let i: TerminalExplainInputs = serde_json::from_str(escaped).unwrap();
        assert_eq!(i.screen_tail, "a\n日");
    }
}
