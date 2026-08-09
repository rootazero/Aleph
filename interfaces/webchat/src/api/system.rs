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

// `SubagentConcurrencyMetrics` / `SubagentConcurrency` / `SessionSubagentCount`
// were the payload mirrors for the cut `SystemApi::subagent_concurrency` above.
// The wire shape they described still exists server-side — see
// `gateway/handlers/gateway_metrics.rs` and `agents/background_tracker.rs::
// subagent_snapshot`, which are the authoritative definitions.
