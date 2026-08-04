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

    /// Round-8 (§4.11) — `gateway.metrics.subagent_concurrency` — live
    /// background-sub-agent occupancy. Mirrors `run_concurrency` for the
    /// `BackgroundAgentTracker` so a panel can render the §4.11 gauge with
    /// the same widget that renders §4.10. Pass `scope = Some(session_key)`
    /// to limit the view to one session.
    pub async fn subagent_concurrency(
        state: &DashboardState,
        scope: Option<&str>,
    ) -> Result<SubagentConcurrencyMetrics, String> {
        let params = match scope {
            Some(s) => Value::Object(
                [("scope".to_string(), Value::String(s.to_string()))]
                    .into_iter()
                    .collect(),
            ),
            None => Value::Null,
        };
        let result = state
            .rpc_call("gateway.metrics.subagent_concurrency", params)
            .await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse subagent concurrency: {e}"))
    }
}

/// Combined payload of `gateway.metrics.run_concurrency`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunConcurrencyMetrics {
    #[serde(default)]
    pub run_concurrency: RunConcurrency,
    /// Backend session keys with a run currently in flight.
    #[serde(default)]
    pub running_sessions: Vec<String>,
    /// Messages parked in the per-session busy wait lanes — the backlog
    /// *behind* the run slots. Absent on servers predating the field.
    #[serde(default)]
    pub busy_queue: BusyQueue,
}

/// Mirror of the server-side `busy_queue::BusyQueueSnapshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusyQueue {
    #[serde(default)]
    pub total_waiting: usize,
    /// Deepest lane first; idle sessions omitted.
    #[serde(default)]
    pub per_session: Vec<SessionQueueDepth>,
}

/// One session's queued-message backlog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionQueueDepth {
    pub session_key: String,
    pub depth: usize,
}

/// Mirror of server-side `ConcurrencySnapshot` (`execution_engine/concurrency.rs`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunConcurrency {
    #[serde(default)]
    pub global_in_use: usize,
    #[serde(default)]
    pub global_total: usize,
    #[serde(default)]
    pub per_agent_cap: usize,
    #[serde(default)]
    pub waiting: usize,
    #[serde(default)]
    pub per_agent: Vec<AgentSlotUsage>,
}

/// Mirror of server-side `AgentSlotUsage` — one agent's live run-slot usage.
/// The agent id is the memory/storage isolation boundary (distinct from the
/// per-session parallelism unit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSlotUsage {
    pub agent_id: String,
    pub in_use: usize,
}

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

/// Round-8 (§4.11) — mirror of the `subagent_concurrency` RPC payload. The
/// `consumed_total / completed_total` ratio is the dedup-hygiene gauge: a
/// high `completed_total` paired with a low `consumed_total` means the
/// parent is ignoring its background results. Mirrors the
/// `{"subagent_concurrency": {...}}` envelope the handler emits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentConcurrencyMetrics {
    #[serde(default)]
    pub subagent_concurrency: SubagentConcurrency,
}

/// Inner sub-agent snapshot — the per-process live occupancy for §4.11.
/// Same shape as `RunConcurrency::per_agent` rows so a panel widget renders
/// both gauges with one row template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentConcurrency {
    #[serde(default)]
    pub running_total: usize,
    #[serde(default)]
    pub running_per_session: Vec<SessionSubagentCount>,
    /// Running entries that are *presence-only* (sync fan-out seams, MoA
    /// aggregators, team-chat members). Excluded from the `subagent` tool's
    /// enumeration faces by design, but they DO count against the parent's
    /// Interrupt-demote budget.
    #[serde(default)]
    pub presence_only_total: usize,
    #[serde(default)]
    pub completed_total: usize,
    #[serde(default)]
    pub consumed_total: usize,
}

/// One session's live sub-agent count, in the same shape as
/// `RunConcurrency::per_agent` so the panel widget renders both gauges with
/// the same row template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSubagentCount {
    pub session: String,
    pub count: usize,
}
