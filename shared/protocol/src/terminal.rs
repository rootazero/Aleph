//! Shared wire contracts for terminal observation.
//!
//! Runtime defaults, ownership, timeout clamping and empty-state rejection
//! belong to Core. These types preserve inputs without adding a second policy.

use crate::runtime::{RuntimeAgentEntry, RuntimeAgentState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalReadResponse {
    pub session_id: String,
    /// Current visible screen text, without scrollback.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWaitOutcome {
    Reached,
    Timeout,
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWaitResponse {
    pub session_id: String,
    pub outcome: TerminalWaitOutcome,
    pub agent: Option<RuntimeAgentEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExplainRule {
    pub id: String,
    pub priority: i32,
    pub region: String,
    pub state: RuntimeAgentState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExplainInputs {
    pub title: String,
    pub osc_progress: String,
    /// Display-only tail; the engine evaluates the entire visible screen.
    pub screen_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExplainResponse {
    pub session_id: String,
    pub agent: Option<String>,
    /// Fresh screen evaluation; it can differ from the runtime's held state.
    pub state: RuntimeAgentState,
    pub matched_rule: Option<TerminalExplainRule>,
    pub source: Option<String>,
    pub manifest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub inputs: TerminalExplainInputs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalSessionParams {
    /// The PTY session id from the caller's session list.
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalWaitParams {
    /// The PTY session id from the caller's session list.
    pub session_id: String,
    /// Agent states that end the wait.
    #[serde(default)]
    pub until: Option<Vec<RuntimeAgentState>>,
    /// Maximum time to wait, in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeAgentEntry, RuntimeAgentState};
    use serde_json::{json, Value};

    // These fixtures pin the legacy wire shapes. Tests of actual Core
    // producers and caller admission belong to the runtime convergence task.
    fn agent() -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: "s1".into(),
            label: "claude".into(),
            cwd: "/workspace".into(),
            agent: Some("claude".into()),
            program: Some("claude".into()),
            state: RuntimeAgentState::Blocked,
            updated_at: 42,
            quiet_since: None,
        }
    }

    #[test]
    fn terminal_read_schema_preserves_legacy_keys() {
        let response = TerminalReadResponse {
            session_id: "s1".into(),
            text: "中🙂\nwaiting".into(),
        };
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire, json!({"session_id": "s1", "text": "中🙂\nwaiting"}));
        assert_eq!(
            serde_json::from_value::<TerminalReadResponse>(wire).unwrap(),
            response
        );
    }

    #[test]
    fn wait_payload_preserves_all_legacy_outcomes_and_runtime_entry() {
        for (outcome, word, entry) in [
            (TerminalWaitOutcome::Reached, "reached", Some(agent())),
            (TerminalWaitOutcome::Timeout, "timeout", Some(agent())),
            (TerminalWaitOutcome::Timeout, "timeout", None),
            (TerminalWaitOutcome::Gone, "gone", None),
        ] {
            let response = TerminalWaitResponse {
                session_id: "s1".into(),
                outcome,
                agent: entry.clone(),
            };
            let wire = serde_json::to_value(&response).unwrap();
            assert_eq!(
                wire,
                json!({"session_id": "s1", "outcome": word, "agent": entry})
            );
            assert_eq!(
                serde_json::from_value::<TerminalWaitResponse>(wire).unwrap(),
                response
            );
        }
        assert!(serde_json::from_value::<TerminalWaitOutcome>(json!("cancelled")).is_err());
    }

    #[test]
    fn explain_preserves_legacy_keys_and_omits_only_absent_reason() {
        let mut response = TerminalExplainResponse {
            session_id: "s1".into(),
            agent: Some("claude".into()),
            state: RuntimeAgentState::Blocked,
            matched_rule: Some(TerminalExplainRule {
                id: "permission".into(),
                priority: 100,
                region: "bottom".into(),
                state: RuntimeAgentState::Blocked,
            }),
            source: Some("bundled".into()),
            manifest_version: Some("v1".into()),
            reason: None,
            inputs: TerminalExplainInputs {
                title: "claude".into(),
                osc_progress: "".into(),
                screen_tail: "Allow tool?".into(),
            },
        };
        let matched = serde_json::to_value(&response).unwrap();
        assert_eq!(
            matched,
            json!({
                "session_id": "s1", "agent": "claude", "state": "blocked",
                "matched_rule": {"id": "permission", "priority": 100,
                    "region": "bottom", "state": "blocked"},
                "source": "bundled", "manifest_version": "v1",
                "inputs": {"title": "claude", "osc_progress": "",
                    "screen_tail": "Allow tool?"}
            })
        );
        assert_eq!(
            serde_json::from_value::<TerminalExplainResponse>(matched).unwrap(),
            response
        );

        response.agent = None;
        response.state = RuntimeAgentState::Unknown;
        response.matched_rule = None;
        response.source = None;
        response.manifest_version = None;
        response.reason = Some("not sampled".into());
        let unknown = serde_json::to_value(&response).unwrap();
        assert_eq!(
            unknown,
            json!({
                "session_id": "s1", "agent": null, "state": "unknown",
                "matched_rule": null, "source": null, "manifest_version": null,
                "reason": "not sampled",
                "inputs": {"title": "claude", "osc_progress": "",
                    "screen_tail": "Allow tool?"}
            })
        );
        assert_eq!(
            serde_json::from_value::<TerminalExplainResponse>(unknown).unwrap(),
            response
        );
    }

    fn collect_refs(value: &Value, refs: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if key == "$ref" {
                        refs.push(value.as_str().unwrap().to_owned());
                    } else {
                        collect_refs(value, refs);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect_refs(value, refs);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn wait_state_uses_runtime_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(TerminalWaitParams)).unwrap();
        let mut refs = Vec::new();
        collect_refs(&schema["properties"]["until"], &mut refs);
        assert_eq!(refs, ["#/$defs/RuntimeAgentState"]);
        assert_eq!(
            schema["$defs"]["RuntimeAgentState"]["enum"],
            json!(["idle", "working", "blocked", "unknown"])
        );
        let runtime = serde_json::to_value(schemars::schema_for!(RuntimeAgentState)).unwrap();
        assert_eq!(
            schema["$defs"]["RuntimeAgentState"]["enum"],
            runtime["enum"]
        );
    }

    #[test]
    fn request_types_preserve_inputs_for_runtime_validation() {
        let params: TerminalWaitParams =
            serde_json::from_value(json!({"session_id": "s1"})).unwrap();
        assert_eq!(params.until, None);
        assert_eq!(params.timeout_ms, None);
        let params: TerminalWaitParams = serde_json::from_value(json!({
            "session_id": "s1", "until": [], "timeout_ms": u64::MAX
        }))
        .unwrap();
        assert_eq!(params.until, Some(vec![]));
        assert_eq!(params.timeout_ms, Some(u64::MAX));
        assert!(serde_json::from_value::<TerminalWaitParams>(json!({
            "session_id": "s1", "until": ["guess"]
        }))
        .is_err());
        assert!(serde_json::from_value::<TerminalWaitParams>(json!({
            "session_id": "s1", "timeout_ms": -1
        }))
        .is_err());
        assert!(serde_json::from_value::<TerminalSessionParams>(json!({})).is_err());
    }
}
