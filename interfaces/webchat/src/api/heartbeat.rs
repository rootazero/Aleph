// API layer for Heartbeat Task RPC methods
// Provides type-safe interfaces for managing heartbeat tasks via Gateway

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::DashboardState;

// ============================================================================
// DTOs
// ============================================================================

/// Heartbeat task info returned by the backend
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatTaskInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub interval_ms: u64,
    #[serde(default)]
    pub probe_tool_name: String,
    #[serde(default)]
    pub probe_trigger_condition: String,
    // State fields
    #[serde(default)]
    pub next_due_ms: Option<i64>,
    #[serde(default)]
    pub last_probe_at_ms: Option<i64>,
    #[serde(default)]
    pub last_l2_status: Option<String>,
    #[serde(default)]
    pub consecutive_errors: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Request payload for creating a heartbeat task
#[derive(Debug, Clone, Serialize)]
pub struct CreateHeartbeatTask {
    pub name: String,
    pub agent_id: String,
    /// Interval string: "5m", "30s", "2h", etc.
    pub interval: String,
    pub probe_tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_trigger_condition: Option<String>,
}

/// Request payload for updating a heartbeat task
#[derive(Debug, Clone, Serialize)]
pub struct UpdateHeartbeatTask {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_trigger_condition: Option<String>,
}

/// Heartbeat run record returned by the backend
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatRunInfo {
    pub id: String,
    #[serde(default)]
    pub trigger_source: String,
    #[serde(default)]
    pub l1_status: String,
    #[serde(default)]
    pub l2_status: Option<String>,
    #[serde(default)]
    pub l1_duration_ms: Option<i64>,
    #[serde(default)]
    pub l2_duration_ms: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub delivery_status: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

// ============================================================================
// HeartbeatApi
// ============================================================================

pub struct HeartbeatApi;

impl HeartbeatApi {
    /// List all heartbeat tasks
    pub async fn list(state: &DashboardState) -> Result<Vec<HeartbeatTaskInfo>, String> {
        let result = state.rpc_call("heartbeat.list", Value::Null).await?;

        result
            .get("tasks")
            .ok_or_else(|| "Invalid response: missing tasks".to_string())
            .and_then(|tasks| {
                serde_json::from_value(tasks.clone())
                    .map_err(|e| format!("Failed to parse heartbeat tasks: {}", e))
            })
    }

    /// Get a single heartbeat task by ID
    pub async fn get(state: &DashboardState, task_id: &str) -> Result<HeartbeatTaskInfo, String> {
        let params = serde_json::json!({ "task_id": task_id });
        let result = state.rpc_call("heartbeat.get", params).await?;

        result
            .get("task")
            .ok_or_else(|| "Invalid response: missing task".to_string())
            .and_then(|task| {
                serde_json::from_value(task.clone())
                    .map_err(|e| format!("Failed to parse heartbeat task: {}", e))
            })
    }

    /// Create a new heartbeat task
    pub async fn create(
        state: &DashboardState,
        req: CreateHeartbeatTask,
    ) -> Result<HeartbeatTaskInfo, String> {
        let params = serde_json::to_value(&req)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let result = state.rpc_call("heartbeat.create", params).await?;

        result
            .get("task")
            .ok_or_else(|| "Invalid response: missing task".to_string())
            .and_then(|task| {
                serde_json::from_value(task.clone())
                    .map_err(|e| format!("Failed to parse created heartbeat task: {}", e))
            })
    }

    /// Update an existing heartbeat task
    pub async fn update(
        state: &DashboardState,
        patch: UpdateHeartbeatTask,
    ) -> Result<HeartbeatTaskInfo, String> {
        let params = serde_json::to_value(&patch)
            .map_err(|e| format!("Failed to serialize patch: {}", e))?;

        let result = state.rpc_call("heartbeat.update", params).await?;

        result
            .get("task")
            .ok_or_else(|| "Invalid response: missing task".to_string())
            .and_then(|task| {
                serde_json::from_value(task.clone())
                    .map_err(|e| format!("Failed to parse updated heartbeat task: {}", e))
            })
    }

    /// Delete a heartbeat task by ID
    pub async fn delete(state: &DashboardState, task_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "task_id": task_id });
        state.rpc_call("heartbeat.delete", params).await?;
        Ok(())
    }

    /// Toggle a heartbeat task enabled/disabled
    pub async fn toggle(
        state: &DashboardState,
        task_id: &str,
        enabled: bool,
    ) -> Result<HeartbeatTaskInfo, String> {
        let params = serde_json::json!({
            "task_id": task_id,
            "enabled": enabled,
        });

        let result = state.rpc_call("heartbeat.toggle", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse toggled heartbeat task: {}", e))
    }

    /// Wake a heartbeat task immediately (trigger L1 probe now)
    pub async fn wake(state: &DashboardState, task_id: &str) -> Result<Value, String> {
        let params = serde_json::json!({ "task_id": task_id });
        state.rpc_call("heartbeat.wake", params).await
    }

    /// Get execution run history for a heartbeat task
    pub async fn runs(
        state: &DashboardState,
        task_id: &str,
        limit: i32,
    ) -> Result<Vec<HeartbeatRunInfo>, String> {
        let params = serde_json::json!({
            "task_id": task_id,
            "limit": limit,
        });

        let result = state.rpc_call("heartbeat.runs", params).await?;

        result
            .get("runs")
            .ok_or_else(|| "Invalid response: missing runs".to_string())
            .and_then(|runs| {
                serde_json::from_value(runs.clone())
                    .map_err(|e| format!("Failed to parse heartbeat runs: {}", e))
            })
    }
}
