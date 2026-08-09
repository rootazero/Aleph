//! Heartbeat task RPC handlers.
//!
//! Handlers for heartbeat monitoring operations: list, get, create, update,
//! delete, toggle, wake, runs.
//!
//! Each method has two variants:
//! - `handle_xxx_stub`: stateless stubs returning fake/empty data (used in `HandlerRegistry::new()`)
//! - `handle_xxx`: real handlers that delegate to `HeartbeatService` via `SharedHeartbeatService`

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::tasks::heartbeat::config::{HeartbeatTaskView, ProbeConfig, TriggerCondition};
use crate::tasks::heartbeat::service::ops::HeartbeatTaskUpdates;
use crate::tasks::heartbeat::wake::{WakePriority, WakeRequest};
use crate::tasks::heartbeat::SharedHeartbeatService;
use crate::tasks::shared::clock::SystemClock;

// ============================================================================
// Helper functions
// ============================================================================

/// Extract a string parameter from a JSON-RPC request
fn extract_str(request: &JsonRpcRequest, key: &str) -> Option<String> {
    match &request.params {
        Some(Value::Object(map)) => map.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

/// Read an optional-and-clearable parameter in the tri-state update
/// convention: absent → `None` (leave alone); explicit `null` → `Some(None)`
/// (clear); value → `Some(Some(cfg))`.
///
/// A parse error is rejected rather than ignored — a monitor whose alerting or
/// delivery silently did not take is the exact failure these fields exist to
/// prevent.
fn parse_tristate<T: serde::de::DeserializeOwned>(
    params: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Option<T>>, String> {
    match params.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => serde_json::from_value(v.clone())
            .map(|cfg| Some(Some(cfg)))
            .map_err(|e| format!("Invalid {key}: {e}")),
    }
}

fn parse_failure_alert(
    params: &serde_json::Map<String, Value>,
) -> Result<Option<Option<crate::tasks::shared::alert::FailureAlertConfig>>, String> {
    parse_tristate(params, "failure_alert")
}

/// Where this monitor's findings go. Nothing wrote `delivery_config` anywhere
/// in the repo until 2026-08-09, so every task ever created ran its L2 turn and
/// then dropped the finding while recording the run as a success.
fn parse_delivery(
    params: &serde_json::Map<String, Value>,
) -> Result<Option<Option<crate::tasks::shared::delivery::DeliveryConfig>>, String> {
    parse_tristate(params, "delivery")
}

/// Serialize a `HeartbeatTaskView` to JSON
fn task_view_to_json(view: &HeartbeatTaskView) -> Value {
    json!({
        "id": view.id,
        "name": view.name,
        "agent_id": view.agent_id,
        "enabled": view.enabled,
        "interval_ms": view.interval_ms,
        "probe": {
            "tool_name": view.probe.tool_name,
            "tool_params": view.probe.tool_params,
            "trigger_condition": view.probe.trigger_condition,
        },
        "active_hours": view.active_hours,
        "failure_alert": view.failure_alert,
        "delivery": view.delivery_config,
        "state": {
            "next_due_ms": view.state.next_due_ms,
            "running_at_ms": view.state.running_at_ms,
            "last_probe_at_ms": view.state.last_probe_at_ms,
            "last_probe_result": view.state.last_probe_result,
            "last_l2_at_ms": view.state.last_l2_at_ms,
            "last_l2_status": view.state.last_l2_status,
            "consecutive_errors": view.state.consecutive_errors,
            "last_error": view.state.last_error,
        },
        "created_at": view.created_at,
        "updated_at": view.updated_at,
    })
}

/// Parse an interval string like "5m", "1h", "30s" into milliseconds.
/// Falls back to parsing as raw milliseconds.
fn parse_interval(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('s') {
        rest.trim()
            .parse::<u64>()
            .map_err(|e| format!("invalid seconds value: {e}"))
            .and_then(|v| {
                v.checked_mul(1_000)
                    .ok_or_else(|| "interval too large".to_string())
            })
    } else if let Some(rest) = s.strip_suffix('m') {
        rest.trim()
            .parse::<u64>()
            .map_err(|e| format!("invalid minutes value: {e}"))
            .and_then(|v| {
                v.checked_mul(60_000)
                    .ok_or_else(|| "interval too large".to_string())
            })
    } else if let Some(rest) = s.strip_suffix('h') {
        rest.trim()
            .parse::<u64>()
            .map_err(|e| format!("invalid hours value: {e}"))
            .and_then(|v| {
                v.checked_mul(3_600_000)
                    .ok_or_else(|| "interval too large".to_string())
            })
    } else {
        s.parse::<u64>()
            .map_err(|e| format!("invalid interval: {e}"))
    }
}

// ============================================================================
// Real handlers (backed by HeartbeatService)
// ============================================================================

/// Handle heartbeat.list RPC request (real)
pub async fn handle_list(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let service = service.lock().await;
    let tasks = service.list_tasks().await;
    let tasks_json: Vec<Value> = tasks.iter().map(task_view_to_json).collect();
    JsonRpcResponse::success(request.id, json!({ "tasks": tasks_json }))
}

/// Handle heartbeat.get RPC request (real)
pub async fn handle_get(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let service = service.lock().await;
    match service.get_task(&task_id).await {
        Some(view) => {
            JsonRpcResponse::success(request.id, json!({ "task": task_view_to_json(&view) }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Task not found: {task_id}"),
        ),
    }
}

/// Handle heartbeat.create RPC request (real)
pub async fn handle_create(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing name");
        }
    };

    let agent_id = params
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();

    // Parse interval: accept interval_ms (number) or interval (string like "5m")
    let interval_ms = if let Some(ms) = params.get("interval_ms").and_then(|v| v.as_u64()) {
        ms
    } else if let Some(s) = params.get("interval").and_then(|v| v.as_str()) {
        match parse_interval(s) {
            Ok(ms) => ms,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid interval: {e}"),
                );
            }
        }
    } else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Missing interval_ms or interval",
        );
    };

    // Parse probe config
    let probe = match params.get("probe") {
        Some(probe_val) => {
            let tool_name = match probe_val.get("tool_name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Missing probe.tool_name",
                    );
                }
            };
            let tool_params = probe_val.get("tool_params").cloned();
            let trigger_condition = match probe_val.get("trigger_condition") {
                Some(tc) => match serde_json::from_value::<TriggerCondition>(tc.clone()) {
                    Ok(cond) => cond,
                    Err(e) => {
                        return JsonRpcResponse::error(
                            request.id,
                            INVALID_PARAMS,
                            format!("Invalid trigger_condition: {e}"),
                        );
                    }
                },
                None => TriggerCondition::Always,
            };
            ProbeConfig {
                tool_name,
                tool_params,
                trigger_condition,
            }
        }
        None => {
            // Fallback: try top-level tool_name
            match params.get("tool_name").and_then(|v| v.as_str()) {
                Some(tn) => ProbeConfig {
                    tool_name: tn.to_string(),
                    tool_params: params.get("tool_params").cloned(),
                    trigger_condition: TriggerCondition::Always,
                },
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Missing probe or tool_name",
                    );
                }
            }
        }
    };

    let mut task =
        crate::tasks::heartbeat::config::HeartbeatTask::new(name, agent_id, interval_ms, probe);

    // Optional: enabled
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        task.enabled = enabled;
    }

    // Optional: active_hours
    if let Some(val) = params.get("active_hours") {
        task.active_hours = serde_json::from_value(val.clone()).ok().flatten();
    }

    // Optional: failure_alert. Rejected rather than ignored on a parse error —
    // a monitor whose alerting silently did not take is the exact failure this
    // field exists to prevent.
    match parse_failure_alert(params) {
        Ok(Some(alert)) => task.failure_alert = alert,
        Ok(None) => {}
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }

    // Optional: delivery. Without it the monitor has nowhere to report to and
    // `resolve_alert_config`'s default failure target is dead too.
    match parse_delivery(params) {
        Ok(Some(delivery)) => task.delivery_config = delivery,
        Ok(None) => {}
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }

    let clock = SystemClock;
    let service = service.lock().await;
    let task_id = match service.add_task(task, &clock).await {
        Ok(id) => id,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to create heartbeat task: {e}"),
            );
        }
    };
    match service.get_task(&task_id).await {
        Some(view) => {
            JsonRpcResponse::success(request.id, json!({ "task": task_view_to_json(&view) }))
        }
        None => JsonRpcResponse::success(request.id, json!({ "task": { "id": task_id } })),
    }
}

/// Handle heartbeat.update RPC request (real)
pub async fn handle_update(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let task_id = match params.get("task_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let mut updates = HeartbeatTaskUpdates::default();

    if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
        updates.name = Some(name.to_string());
    }
    if let Some(agent_id) = params.get("agent_id").and_then(|v| v.as_str()) {
        updates.agent_id = Some(agent_id.to_string());
    }
    if let Some(ms) = params.get("interval_ms").and_then(|v| v.as_u64()) {
        updates.interval_ms = Some(ms);
    } else if let Some(s) = params.get("interval").and_then(|v| v.as_str()) {
        match parse_interval(s) {
            Ok(ms) => updates.interval_ms = Some(ms),
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid interval: {e}"),
                );
            }
        }
    }
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        updates.enabled = Some(enabled);
    }
    if let Some(probe_val) = params.get("probe") {
        match serde_json::from_value::<ProbeConfig>(probe_val.clone()) {
            Ok(probe) => updates.probe = Some(probe),
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid probe: {e}"),
                );
            }
        }
    }
    if let Some(val) = params.get("active_hours") {
        updates.active_hours = serde_json::from_value(val.clone()).ok();
    }
    match parse_failure_alert(params) {
        Ok(alert) => updates.failure_alert = alert,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }
    match parse_delivery(params) {
        Ok(delivery) => updates.delivery_config = delivery,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }

    let clock = SystemClock;
    let service = service.lock().await;
    match service.update_task(&task_id, updates, &clock).await {
        Ok(()) => match service.get_task(&task_id).await {
            Some(view) => {
                JsonRpcResponse::success(request.id, json!({ "task": task_view_to_json(&view) }))
            }
            None => JsonRpcResponse::success(
                request.id,
                json!({ "task": { "id": task_id, "updated": true } }),
            ),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update task: {e}"),
        ),
    }
}

/// Handle heartbeat.delete RPC request (real)
pub async fn handle_delete(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let service = service.lock().await;
    match service.delete_task(&task_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "deleted": task_id })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete task: {e}"),
        ),
    }
}

/// Handle heartbeat.toggle RPC request (real)
pub async fn handle_toggle(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let task_id = match params.get("task_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let clock = SystemClock;
    let service = service.lock().await;

    // If explicit enabled value is provided, set it; otherwise toggle
    let result = if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        let updates = HeartbeatTaskUpdates {
            enabled: Some(enabled),
            ..Default::default()
        };
        service
            .update_task(&task_id, updates, &clock)
            .await
            .map(|()| enabled)
    } else {
        service.toggle_task(&task_id, &clock).await
    };

    match result {
        Ok(new_enabled) => JsonRpcResponse::success(
            request.id,
            json!({
                "task_id": task_id,
                "enabled": new_enabled,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to toggle task: {e}"),
        ),
    }
}

/// Handle heartbeat.wake RPC request (real)
///
/// Manually triggers a heartbeat task by enqueuing a `WakeRequest` with `UserAction` priority.
pub async fn handle_wake(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let reason = match &request.params {
        Some(Value::Object(map)) => map
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    let service = service.lock().await;
    // Verify task exists
    match service.get_task(&task_id).await {
        Some(_) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            service.wake_queue().enqueue(WakeRequest {
                task_id: task_id.clone(),
                priority: WakePriority::UserAction,
                reason: reason.clone(),
                requested_at: now_ms,
            });
            JsonRpcResponse::success(
                request.id,
                json!({
                    "triggered": task_id,
                    "status": "queued",
                    "reason": reason,
                }),
            )
        }
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Task not found: {task_id}"),
        ),
    }
}

/// Handle heartbeat.runs RPC request (real)
///
/// Returns the execution history from `SQLite`.
pub async fn handle_runs(
    request: JsonRpcRequest,
    service: SharedHeartbeatService,
) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let limit = match &request.params {
        Some(Value::Object(map)) => {
            map.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize
        }
        _ => 20,
    };

    let service = service.lock().await;
    let store = service.state().store.lock().await;
    match store.get_runs(&task_id, limit) {
        Ok(runs) => {
            let runs_json: Vec<Value> = runs
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "task_id": r.task_id,
                        "trigger_source": r.trigger_source,
                        "l1_status": r.l1_status,
                        "l2_status": r.l2_status,
                        "started_at": r.started_at,
                        "ended_at": r.ended_at,
                        "l1_duration_ms": r.l1_duration_ms,
                        "l2_duration_ms": r.l2_duration_ms,
                        "error": r.error,
                        "delivery_status": r.delivery_status,
                        "created_at": r.created_at,
                    })
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "task_id": task_id, "runs": runs_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get runs: {e}"),
        ),
    }
}

// ============================================================================
// Stub handlers (stateless, for HandlerRegistry::new())
// ============================================================================

/// Handle heartbeat.list RPC request (stub)
pub async fn handle_list_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(request.id, json!({ "tasks": [] }))
}

/// Handle heartbeat.get RPC request (stub)
pub async fn handle_get_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "task": {
                "id": task_id,
                "name": "",
                "enabled": false,
                "interval_ms": 0,
                "created_at": null,
                "updated_at": null
            }
        }),
    )
}

/// Handle heartbeat.create RPC request (stub)
pub async fn handle_create_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed");

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    JsonRpcResponse::success(
        request.id,
        json!({
            "task": {
                "id": id,
                "name": name,
                "enabled": true,
                "created_at": now,
                "updated_at": now
            }
        }),
    )
}

/// Handle heartbeat.update RPC request (stub)
pub async fn handle_update_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "task": { "id": task_id, "updated": true } }),
    )
}

/// Handle heartbeat.delete RPC request (stub)
pub async fn handle_delete_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    JsonRpcResponse::success(request.id, json!({ "deleted": task_id }))
}

/// Handle heartbeat.toggle RPC request (stub)
pub async fn handle_toggle_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let task_id = match params.get("task_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let enabled = params
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    JsonRpcResponse::success(
        request.id,
        json!({
            "task_id": task_id,
            "enabled": enabled
        }),
    )
}

/// Handle heartbeat.wake RPC request (stub)
pub async fn handle_wake_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "triggered": task_id,
            "status": "queued"
        }),
    )
}

/// Handle heartbeat.runs RPC request (stub)
pub async fn handle_runs_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let task_id = match extract_str(&request, "task_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "task_id": task_id,
            "runs": []
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_list_stub() {
        let request = JsonRpcRequest::with_id("heartbeat.list", None, json!(1));
        let response = handle_list_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.get",
            Some(json!({ "task_id": "check-gmail" })),
            Some(json!(1)),
        );
        let response = handle_get_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub_missing_task_id() {
        let request = JsonRpcRequest::with_id("heartbeat.get", None, json!(1));
        let response = handle_get_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.create",
            Some(json!({ "name": "check-gmail", "interval_ms": 300000 })),
            Some(json!(1)),
        );
        let response = handle_create_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_create_stub_missing_params() {
        let request = JsonRpcRequest::with_id("heartbeat.create", None, json!(1));
        let response = handle_create_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_update_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.update",
            Some(json!({ "task_id": "check-gmail", "name": "updated" })),
            Some(json!(1)),
        );
        let response = handle_update_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_update_stub_missing_task_id() {
        let request = JsonRpcRequest::with_id("heartbeat.update", None, json!(1));
        let response = handle_update_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_delete_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.delete",
            Some(json!({ "task_id": "check-gmail" })),
            Some(json!(1)),
        );
        let response = handle_delete_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_delete_stub_missing_task_id() {
        let request = JsonRpcRequest::with_id("heartbeat.delete", None, json!(1));
        let response = handle_delete_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_toggle_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.toggle",
            Some(json!({ "task_id": "check-gmail", "enabled": false })),
            Some(json!(1)),
        );
        let response = handle_toggle_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_toggle_stub_missing_params() {
        let request = JsonRpcRequest::with_id("heartbeat.toggle", None, json!(1));
        let response = handle_toggle_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_wake_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.wake",
            Some(json!({ "task_id": "check-gmail" })),
            Some(json!(1)),
        );
        let response = handle_wake_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_wake_stub_missing_task_id() {
        let request = JsonRpcRequest::with_id("heartbeat.wake", None, json!(1));
        let response = handle_wake_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_runs_stub() {
        let request = JsonRpcRequest::new(
            "heartbeat.runs",
            Some(json!({ "task_id": "check-gmail" })),
            Some(json!(1)),
        );
        let response = handle_runs_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_runs_stub_missing_task_id() {
        let request = JsonRpcRequest::with_id("heartbeat.runs", None, json!(1));
        let response = handle_runs_stub(request).await;
        assert!(response.is_error());
    }

    #[test]
    fn test_parse_interval() {
        assert_eq!(parse_interval("5m").unwrap(), 300_000);
        assert_eq!(parse_interval("1h").unwrap(), 3_600_000);
        assert_eq!(parse_interval("30s").unwrap(), 30_000);
        assert_eq!(parse_interval("60000").unwrap(), 60_000);
        assert!(parse_interval("abc").is_err());
    }
}
