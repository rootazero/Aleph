//! Trace API — wraps trace.list / trace.get RPC methods.

use crate::context::DashboardState;
use aleph_protocol::{AgentTraceReplay, AgentTraceReplayListItem};

pub struct TraceApi;

impl TraceApi {
    /// Fetch the list of available trace replays.
    pub async fn list(state: &DashboardState) -> Result<Vec<AgentTraceReplayListItem>, String> {
        let result = state.rpc_call("trace.list", serde_json::json!({})).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse trace list: {}", e))
    }

    /// Fetch a full trace replay for a given task.
    pub async fn get(state: &DashboardState, task_id: &str) -> Result<AgentTraceReplay, String> {
        let result = state
            .rpc_call("trace.get", serde_json::json!({ "task_id": task_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse trace: {}", e))
    }
}
