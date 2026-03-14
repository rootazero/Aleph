//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, get, create, update, delete,
//! status, run, runs, toggle.
//!
//! Each method has two variants:
//! - `handle_xxx_stub`: stateless stubs returning fake/empty data (used in HandlerRegistry::new())
//! - `handle_xxx`: real handlers that delegate to `CronService` via `SharedCronService`

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::cron::clock::Clock;
use crate::cron::{
    CronJob, CronJobView, ScheduleKind, SharedCronService,
    SessionTarget,
};
use crate::cron::service::ops::CronJobUpdates;

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

/// Serialize a CronJobView to JSON (includes all new fields)
fn job_view_to_json(view: &CronJobView) -> Value {
    json!({
        "id": view.id,
        "name": view.name,
        "enabled": view.enabled,
        "schedule_kind": view.schedule_kind,
        "agent_id": view.agent_id,
        "prompt": view.prompt,
        "timezone": view.timezone,
        "tags": view.tags,
        "session_target": view.session_target,
        "created_at": view.created_at,
        "updated_at": view.updated_at,
        // State fields
        "next_run_at_ms": view.state.next_run_at_ms,
        "running_at_ms": view.state.running_at_ms,
        "last_run_at_ms": view.state.last_run_at_ms,
        "last_run_status": view.state.last_run_status,
        "last_error": view.state.last_error,
        "last_error_reason": view.state.last_error_reason,
        "last_duration_ms": view.state.last_duration_ms,
        "consecutive_errors": view.state.consecutive_errors,
        "last_delivery_status": view.state.last_delivery_status,
        // Config fields
        "delivery_config": view.delivery_config,
        "failure_alert": view.failure_alert,
    })
}

// ============================================================================
// Real handlers (backed by CronService)
// ============================================================================

/// Handle cron.list RPC request (real)
pub async fn handle_list(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let jobs_json: Vec<Value> = jobs.iter().map(job_view_to_json).collect();
            JsonRpcResponse::success(request.id, json!({ "jobs": jobs_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list jobs: {}", e),
        ),
    }
}

/// Handle cron.get RPC request (real)
pub async fn handle_get(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    match service.get_job(&job_id).await {
        Ok(view) => JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get job: {}", e),
        ),
    }
}

/// Handle cron.create RPC request (real)
pub async fn handle_create(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
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

    let prompt = params
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Parse schedule_kind from tagged JSON
    let schedule_kind = match params.get("schedule_kind") {
        Some(sk) => match serde_json::from_value::<ScheduleKind>(sk.clone()) {
            Ok(kind) => kind,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid schedule_kind: {}", e),
                );
            }
        },
        None => {
            // Fallback: try legacy "schedule" field as cron expression
            match params.get("schedule").and_then(|v| v.as_str()) {
                Some(expr) => ScheduleKind::Cron {
                    expr: expr.to_string(),
                    tz: None,
                    stagger_ms: None,
                },
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Missing schedule_kind or schedule",
                    );
                }
            }
        }
    };

    let mut job = CronJob::new(name, agent_id, prompt, schedule_kind);

    // Optional fields
    if let Some(tz) = params.get("timezone").and_then(|v| v.as_str()) {
        job.timezone = Some(tz.to_string());
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        job.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(st) = params.get("session_target") {
        if let Ok(target) = serde_json::from_value::<SessionTarget>(st.clone()) {
            job.session_target = target;
        }
    }

    let service = cron.lock().await;
    match service.add_job(job).await {
        Ok(job_id) => match service.get_job(&job_id).await {
            Ok(view) => {
                JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) }))
            }
            Err(_) => {
                JsonRpcResponse::success(request.id, json!({ "job": { "id": job_id } }))
            }
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create job: {}", e),
        ),
    }
}

/// Handle cron.update RPC request (real)
pub async fn handle_update(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    // Build partial updates
    let mut updates = CronJobUpdates::default();

    if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
        updates.name = Some(name.to_string());
    }
    if let Some(agent_id) = params.get("agent_id").and_then(|v| v.as_str()) {
        updates.agent_id = Some(agent_id.to_string());
    }
    if let Some(prompt) = params.get("prompt").and_then(|v| v.as_str()) {
        updates.prompt = Some(prompt.to_string());
    }
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        updates.enabled = Some(enabled);
    }
    if let Some(sk) = params.get("schedule_kind") {
        if let Ok(kind) = serde_json::from_value::<ScheduleKind>(sk.clone()) {
            updates.schedule_kind = Some(kind);
        }
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        updates.tags = Some(
            tags.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect(),
        );
    }
    if let Some(tz) = params.get("timezone").and_then(|v| v.as_str()) {
        updates.timezone = Some(tz.to_string());
    }

    let service = cron.lock().await;
    match service.update_job(&job_id, updates).await {
        Ok(()) => match service.get_job(&job_id).await {
            Ok(view) => {
                JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) }))
            }
            Err(_) => JsonRpcResponse::success(
                request.id,
                json!({ "job": { "id": job_id, "updated": true } }),
            ),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update job: {}", e),
        ),
    }
}

/// Handle cron.delete RPC request (real)
pub async fn handle_delete(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    match service.delete_job(&job_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "deleted": job_id })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete job: {}", e),
        ),
    }
}

/// Handle cron.status RPC request (real)
pub async fn handle_status(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let enabled_count = jobs.iter().filter(|j| j.enabled).count();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "running": true,
                    "job_count": jobs.len(),
                    "enabled_count": enabled_count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get status: {}", e),
        ),
    }
}

/// Handle cron.run RPC request (real)
///
/// Manually triggers a cron job by setting its next_run_at_ms to now.
pub async fn handle_run(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    // Verify job exists
    match service.get_job(&job_id).await {
        Ok(_view) => {
            // Set next_run_at to now to trigger immediate execution by the timer loop
            let state = service.state();
            let clock_now = state.clock.now_ms();
            let mut store = state.store.lock().await;
            match store.get_job_mut(&job_id) {
                Some(job) => {
                    job.state.next_run_at_ms = Some(clock_now);
                    let _ = store.persist();
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "triggered": job_id,
                            "status": "queued",
                            "next_run_at_ms": clock_now,
                        }),
                    )
                }
                None => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Job not found: {}", job_id),
                ),
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get job: {}", e),
        ),
    }
}

/// Handle cron.runs RPC request (real)
///
/// Returns the execution history — placeholder since run history
/// is not yet persisted in the JSON store.
pub async fn handle_runs(request: JsonRpcRequest, _cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    // Run history is not yet persisted in the JSON store.
    // This will be implemented in the SQLite history migration (Task 21).
    JsonRpcResponse::success(
        request.id,
        json!({
            "job_id": job_id,
            "runs": [],
            "note": "Run history not yet available in JSON store. Coming in Task 21."
        }),
    )
}

/// Handle cron.toggle RPC request (real)
pub async fn handle_toggle(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let enabled = params.get("enabled").and_then(|v| v.as_bool());

    let service = cron.lock().await;
    let result = match enabled {
        Some(true) => service.enable_job(&job_id).await.map(|()| true),
        Some(false) => service.disable_job(&job_id).await.map(|()| false),
        None => service.toggle_job(&job_id).await,
    };

    match result {
        Ok(new_enabled) => JsonRpcResponse::success(
            request.id,
            json!({
                "job_id": job_id,
                "enabled": new_enabled,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to toggle job: {}", e),
        ),
    }
}

// ============================================================================
// Stub handlers (stateless, for HandlerRegistry::new())
// ============================================================================

/// Handle cron.list RPC request (stub)
pub async fn handle_list_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(request.id, json!({ "jobs": [] }))
}

/// Handle cron.get RPC request (stub)
pub async fn handle_get_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "job": {
                "id": job_id,
                "name": "",
                "schedule_kind": { "kind": "cron", "expr": "" },
                "enabled": false,
                "created_at": null,
                "updated_at": null
            }
        }),
    )
}

/// Handle cron.create RPC request (stub)
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
            "job": {
                "id": id,
                "name": name,
                "enabled": true,
                "created_at": now,
                "updated_at": now
            }
        }),
    )
}

/// Handle cron.update RPC request (stub)
pub async fn handle_update_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "job": { "id": job_id, "updated": true } }),
    )
}

/// Handle cron.delete RPC request (stub)
pub async fn handle_delete_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    JsonRpcResponse::success(request.id, json!({ "deleted": job_id }))
}

/// Handle cron.status RPC request (stub)
pub async fn handle_status_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "job_count": 0,
            "last_tick": null
        }),
    )
}

/// Handle cron.run RPC request (stub)
pub async fn handle_run_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "triggered": job_id,
            "status": "queued"
        }),
    )
}

/// Handle cron.runs RPC request (stub)
pub async fn handle_runs_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "job_id": job_id,
            "runs": []
        }),
    )
}

/// Handle cron.toggle RPC request (stub)
pub async fn handle_toggle_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let enabled = params
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    JsonRpcResponse::success(
        request.id,
        json!({
            "job_id": job_id,
            "enabled": enabled
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_list_stub() {
        let request = JsonRpcRequest::with_id("cron.list", None, json!(1));
        let response = handle_list_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub() {
        let request = JsonRpcRequest::new(
            "cron.get",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_get_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.get", None, json!(1));
        let response = handle_get_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create_stub() {
        let request = JsonRpcRequest::new(
            "cron.create",
            Some(json!({ "name": "daily-backup", "schedule": "0 0 * * *" })),
            Some(json!(1)),
        );
        let response = handle_create_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_create_stub_missing_params() {
        let request = JsonRpcRequest::with_id("cron.create", None, json!(1));
        let response = handle_create_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_update_stub() {
        let request = JsonRpcRequest::new(
            "cron.update",
            Some(json!({ "job_id": "daily-backup", "name": "updated" })),
            Some(json!(1)),
        );
        let response = handle_update_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_update_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.update", None, json!(1));
        let response = handle_update_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_delete_stub() {
        let request = JsonRpcRequest::new(
            "cron.delete",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_delete_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_delete_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.delete", None, json!(1));
        let response = handle_delete_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_status_stub() {
        let request = JsonRpcRequest::with_id("cron.status", None, json!(1));
        let response = handle_status_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run_stub() {
        let request = JsonRpcRequest::new(
            "cron.run",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_run_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.run", None, json!(1));
        let response = handle_run_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_runs_stub() {
        let request = JsonRpcRequest::new(
            "cron.runs",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_runs_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_runs_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.runs", None, json!(1));
        let response = handle_runs_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_toggle_stub() {
        let request = JsonRpcRequest::new(
            "cron.toggle",
            Some(json!({ "job_id": "daily-backup", "enabled": false })),
            Some(json!(1)),
        );
        let response = handle_toggle_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_toggle_stub_missing_params() {
        let request = JsonRpcRequest::with_id("cron.toggle", None, json!(1));
        let response = handle_toggle_stub(request).await;
        assert!(response.is_error());
    }
}
