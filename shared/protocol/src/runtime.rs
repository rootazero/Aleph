//! Wire types for the `runtime.*` surface (agent panel).
//!
//! These live here — not in the server — because both the gateway and the
//! two clients depend on this crate, and the server MUST construct its
//! responses from these types rather than hand-rolled `json!` (judgment §10).

use serde::{Deserialize, Serialize};

pub const RUNTIME_AGENTS_CHANGED_TOPIC: &str = "runtime.agents.changed";

/// The RPC method name for the agent panel's snapshot fetch.
///
/// Used by every CLIENT that calls it (currently the TUI's
/// `refresh_runtime_agents`). Deliberately NOT used at the server's own
/// `registry.register(...)` site (`gateway::handlers::mod`): that scanner
/// (`gateway::method_census::sweep_rpc_methods`) requires a STRING LITERAL
/// as the first argument to add a method to its census — passing this
/// constant there would make the registration invisible to the sweep
/// (`literal_after_paren` returns `None` for an identifier) and redden
/// `every_registered_rpc_method_has_a_recorded_ruling`'s staleness check.
/// The server's literal and `RPC_METHOD_CENSUS`'s own entry stay as
/// independently-written literals on purpose — the census's whole value is
/// that it is not derived from what it is checking.
pub const RUNTIME_AGENTS_LIST_METHOD: &str = "runtime.agents.list";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeAgentState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgentEntry {
    pub session_id: String,
    pub label: String,
    /// The directory the session was SPAWNED in; empty when the spawn
    /// inherited the server's. NOT the live cwd — tracking a shell that has
    /// since `cd`'d needs PID probing, which is a phase 0-A gap.
    pub cwd: String,
    /// `None` = the bundled manifest does not recognise this program.
    /// Never a guess.
    pub agent: Option<String>,
    pub state: RuntimeAgentState,
    /// Unix epoch MILLISECONDS, from the sampler's flush-tick clock — not the
    /// client's. Advances only when something observable changed (state,
    /// agent, label or cwd), so an unchanged entry keeps its old value: read
    /// it as "how long has it been like this", not as "when was this last
    /// looked at".
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgentsListResponse {
    pub agents: Vec<RuntimeAgentEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用类型**构造**再解析。只解析一份自己刚写下的字面量，
    /// 测的是 serde 而不是这段代码——那种测试永远绿（判据 §10）。
    #[test]
    fn the_response_round_trips_through_its_own_type() {
        let resp = RuntimeAgentsListResponse {
            agents: vec![RuntimeAgentEntry {
                session_id: "s1".into(),
                label: "claude".into(),
                cwd: "/tmp".into(),
                agent: Some("claude".into()),
                state: RuntimeAgentState::Blocked,
                updated_at: 42,
            }],
        };
        let wire = serde_json::to_value(&resp).unwrap();
        let back: RuntimeAgentsListResponse = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(back, resp);
        assert_eq!(wire["agents"][0]["state"], "blocked");
    }

    /// manifest 不认识它 ⇒ agent 是 None，而不是一个猜出来的名字。
    #[test]
    fn an_unrecognised_agent_serialises_as_null_not_a_guess() {
        let e = RuntimeAgentEntry {
            session_id: "s2".into(),
            label: "zsh".into(),
            cwd: "/tmp".into(),
            agent: None,
            state: RuntimeAgentState::Unknown,
            updated_at: 0,
        };
        assert!(serde_json::to_value(&e).unwrap()["agent"].is_null());
    }
}
