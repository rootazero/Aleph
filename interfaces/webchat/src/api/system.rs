use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    pub platform: String,
    #[serde(default)]
    pub cpu_usage_percent: f32,
    #[serde(default)]
    pub cpu_count: usize,
    #[serde(default)]
    pub memory_used_bytes: u64,
    #[serde(default)]
    pub memory_total_bytes: u64,
    #[serde(default)]
    pub disk_used_bytes: u64,
    #[serde(default)]
    pub disk_total_bytes: u64,
}

pub struct SystemApi;

impl SystemApi {
    /// Get system information
    pub async fn info(state: &DashboardState) -> Result<SystemInfo, String> {
        let result = state.rpc_call("system.info", Value::Null).await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse system info: {e}"))
    }

    /// `gateway.metrics.lanes` — live per-lane occupancy gauge.
    /// Single round-trip; safe to poll on a slow tick (the snapshot is
    /// cheap but eventually consistent).
    pub async fn lane_metrics(state: &DashboardState) -> Result<Vec<LaneOccupancy>, String> {
        let result = state.rpc_call("gateway.metrics.lanes", Value::Null).await?;
        let lanes = result.get("lanes").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(lanes).map_err(|e| format!("Failed to parse lanes: {e}"))
    }

    /// `gateway.metrics.run_concurrency` — run-slot occupancy (global N/M,
    /// per-agent breakdown, queue depth) plus the authoritative set of backend
    /// session keys with a run currently in flight. Cheap, eventually
    /// consistent; the `running_sessions` half lets the sidebar paint
    /// per-session running indicators on a fresh load / for runs started by
    /// any interface, which client-side run-event refcounting alone can't see.
    pub async fn run_concurrency(state: &DashboardState) -> Result<RunConcurrencyMetrics, String> {
        let result = state
            .rpc_call("gateway.metrics.run_concurrency", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse run concurrency: {e}"))
    }

    // A `subagent_concurrency()` mirror of `gateway.metrics.subagent_concurrency`
    // lived here, with its two payload structs, from Round-8 (§4.11) until
    // 2026-08-09. Nothing in `interfaces/` ever called it — no view, no widget,
    // no polling tick — and a `pub` item in this crate never trips `dead_code`,
    // so it read as shipped. The server half is NOT dead and stays: it is an
    // admin-gated read-only diagnostic RPC that scripts and operators reach
    // directly. Cut rather than left in place because a display-only mirror
    // that cannot name the line of code rendering it is scaffolding, and the
    // day a panel wants this gauge, writing the four-line call is cheaper than
    // having carried a stale one (`run_concurrency` above is the live shape to
    // copy — it has three call sites).
}

/// The `gateway.metrics.run_concurrency` payload, as one shape shared with the
/// server that builds it.
///
/// These were five hand-written mirrors of a `serde_json::json!` envelope. A
/// mirror can only ever prove it reads a superset of what the literal happens
/// to emit, so a key renamed server-side degraded to a `#[serde(default)]`
/// zero here — a saturated engine rendering as an idle one (criterion #10).
/// `RunConcurrency::per_agent` is the sharpest case: it is `Option` on the
/// wire because "withheld from this caller" and "no agent is busy" are
/// different answers, and the mirror collapsed both into an empty `Vec`.
pub use aleph_protocol::metrics::{
    AgentSlotUsage, BusyQueueMetrics, RunConcurrency, RunConcurrencyMetrics, SessionQueueDepth,
};

/// Mirror of server-side `LaneOccupancy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneOccupancy {
    pub lane: String,
    #[serde(default)]
    pub desktop_total: Option<usize>,
    #[serde(default)]
    pub desktop_available: Option<usize>,
    pub shared_total: usize,
    pub shared_available: usize,
}

// `SubagentConcurrencyMetrics` / `SubagentConcurrency` / `SessionSubagentCount`
// were the payload mirrors for the cut `SystemApi::subagent_concurrency` above.
// The wire shape they described still exists server-side — see
// `gateway/handlers/gateway_metrics.rs` and `agents/background_tracker.rs::
// subagent_snapshot`, which are the authoritative definitions.

#[cfg(test)]
mod tests {
    use super::*;

    /// Built from the shared type and read back through it — the round trip the
    /// old literal-versus-mirror test could not perform, because both halves of
    /// it were written in this file.
    #[test]
    fn run_concurrency_decodes_the_protocol_type() {
        let sent = RunConcurrencyMetrics {
            run_concurrency: RunConcurrency {
                global_in_use: 3,
                global_total: 8,
                per_agent_cap: 2,
                waiting: 1,
                per_agent: Some(vec![AgentSlotUsage {
                    agent_id: "main".into(),
                    in_use: 2,
                }]),
            },
            running_sessions: vec!["agent:main:main:s1".into()],
            busy_queue: BusyQueueMetrics {
                total_waiting: 4,
                per_session: vec![SessionQueueDepth {
                    session_key: "agent:main:main:s1".into(),
                    depth: 4,
                }],
            },
        };
        let wire = serde_json::to_value(&sent).expect("the server's envelope serialises");
        let parsed: RunConcurrencyMetrics =
            serde_json::from_value(wire).expect("and the Panel parses it");
        assert_eq!(parsed, sent);
    }

    /// The two answers this gauge must not confuse. `None` is "not shown to
    /// you"; `Some([])` is "no agent holds a slot". A reader that rendered the
    /// first as the second would report an idle server off a fact it was simply
    /// not given (criterion #17).
    #[test]
    fn a_withheld_per_agent_breakdown_is_not_an_idle_one() {
        let withheld: RunConcurrency =
            serde_json::from_value(serde_json::json!({ "global_total": 8 }))
                .expect("a member's view parses");
        assert!(withheld.per_agent.is_none());

        let idle: RunConcurrency =
            serde_json::from_value(serde_json::json!({ "global_total": 8, "per_agent": [] }))
                .expect("an operator's view parses");
        assert_eq!(idle.per_agent, Some(Vec::new()));
    }
}
