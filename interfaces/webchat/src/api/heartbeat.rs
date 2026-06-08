// API layer for Heartbeat Task RPC methods
// Provides type-safe interfaces for managing heartbeat tasks via Gateway

use serde::Deserialize;
use serde_json::Value;

use crate::context::DashboardState;

// ============================================================================
// DTOs
// ============================================================================

/// Probe config — nested under `probe` in the backend wire contract
/// (`src/gateway/handlers/heartbeat.rs` `task_view_to_json`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HeartbeatProbe {
    #[serde(default)]
    pub tool_name: String,
    /// Backend `TriggerCondition` is an adjacently-tagged enum
    /// (`{ "type": "Always" }`, `{ "type": "Contains", "value": "x" }`).
    /// Kept as a raw value so the panel round-trips parameterized variants
    /// without lossy stringification.
    #[serde(default)]
    pub trigger_condition: Option<Value>,
}

/// Runtime state — nested under `state` in the backend wire contract.
/// Only the fields the dashboard renders are declared (serde ignores the rest).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HeartbeatState {
    #[serde(default)]
    pub next_due_ms: Option<i64>,
    #[serde(default)]
    pub last_l2_status: Option<String>,
    #[serde(default)]
    pub consecutive_errors: u32,
}

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
    pub probe: HeartbeatProbe,
    #[serde(default)]
    pub state: HeartbeatState,
}

/// Request payload for creating a heartbeat task. Built by the editor with
/// free-text probe fields; the API layer reshapes it into the backend's nested
/// `probe` object before sending.
#[derive(Debug, Clone)]
pub struct CreateHeartbeatTask {
    pub name: String,
    pub agent_id: String,
    /// Interval string: "5m", "30s", "2h", etc.
    pub interval: String,
    pub probe_tool_name: String,
    pub probe_trigger_condition: Option<String>,
}

/// Request payload for updating a heartbeat task
#[derive(Debug, Clone)]
pub struct UpdateHeartbeatTask {
    pub task_id: String,
    pub name: Option<String>,
    pub interval: Option<String>,
    pub enabled: Option<bool>,
    pub probe_tool_name: Option<String>,
    pub probe_trigger_condition: Option<String>,
}

/// Map the editor's free-text trigger condition to the backend's tagged
/// `TriggerCondition` JSON. Empty / unrecognized input defaults to `Always`
/// (fire L2 on every probe), matching the backend default. Recognized forms
/// (case-insensitive): `always`, `non_empty`, `changed`, `contains:<text>`,
/// `>N` / `gt:N`.
fn form_to_trigger_condition(s: &str) -> Value {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("contains:") {
        return serde_json::json!({ "type": "Contains", "value": rest.trim() });
    }
    let gt = t.strip_prefix('>').or_else(|| {
        let lower = t.to_lowercase();
        lower.strip_prefix("gt:").map(|_| &t[3..])
    });
    if let Some(rest) = gt {
        if let Ok(n) = rest.trim().parse::<f64>() {
            return serde_json::json!({ "type": "GreaterThan", "value": n });
        }
    }
    match t.to_lowercase().as_str() {
        "non_empty" | "nonempty" => serde_json::json!({ "type": "NonEmpty" }),
        "changed" => serde_json::json!({ "type": "Changed" }),
        _ => serde_json::json!({ "type": "Always" }),
    }
}

/// Inverse of [`form_to_trigger_condition`] for prefilling the editor from a
/// task's stored `TriggerCondition`.
pub fn trigger_condition_to_form(v: &Value) -> String {
    match v.get("type").and_then(|k| k.as_str()).unwrap_or("Always") {
        "NonEmpty" => "non_empty".to_string(),
        "Changed" => "changed".to_string(),
        "Contains" => format!(
            "contains:{}",
            v.get("value").and_then(|x| x.as_str()).unwrap_or("")
        ),
        "GreaterThan" => format!(
            ">{}",
            v.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0)
        ),
        _ => "always".to_string(),
    }
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
        let params = serde_json::json!({
            "name": req.name,
            "agent_id": req.agent_id,
            "interval": req.interval,
            "probe": {
                "tool_name": req.probe_tool_name,
                "trigger_condition": form_to_trigger_condition(
                    req.probe_trigger_condition.as_deref().unwrap_or_default()
                ),
            },
        });

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
        let mut params = serde_json::Map::new();
        params.insert("task_id".into(), Value::String(patch.task_id));
        if let Some(name) = patch.name {
            params.insert("name".into(), Value::String(name));
        }
        if let Some(interval) = patch.interval {
            params.insert("interval".into(), Value::String(interval));
        }
        if let Some(enabled) = patch.enabled {
            params.insert("enabled".into(), Value::Bool(enabled));
        }
        // The backend reads the whole probe as a `ProbeConfig`; only send it
        // when the editor supplied a tool name.
        if let Some(tool_name) = patch.probe_tool_name {
            params.insert(
                "probe".into(),
                serde_json::json!({
                    "tool_name": tool_name,
                    "trigger_condition": form_to_trigger_condition(
                        patch.probe_trigger_condition.as_deref().unwrap_or_default()
                    ),
                }),
            );
        }

        let result = state.rpc_call("heartbeat.update", Value::Object(params)).await?;

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
