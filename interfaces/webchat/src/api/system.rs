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

        serde_json::from_value(result).map_err(|e| format!("Failed to parse system info: {}", e))
    }

    /// `gateway.metrics.lanes` — live per-lane occupancy gauge.
    /// Single round-trip; safe to poll on a slow tick (the snapshot is
    /// cheap but eventually consistent).
    pub async fn lane_metrics(state: &DashboardState) -> Result<Vec<LaneOccupancy>, String> {
        let result = state.rpc_call("gateway.metrics.lanes", Value::Null).await?;
        let lanes = result.get("lanes").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(lanes).map_err(|e| format!("Failed to parse lanes: {}", e))
    }
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
