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
use crate::cron::{CronJob, CronService, JobRun};
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;

/// Shared CronService handle for real handlers
pub type SharedCronService = Arc<Mutex<CronService>>;

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

/// Serialize a CronJob to JSON
fn job_to_json(job: &CronJob) -> Value {
    json!({
        "id": job.id,
        "name": job.name,
        "schedule": job.schedule,
        "agent_id": job.agent_id,
        "prompt": job.prompt,
        "enabled": job.enabled,
        "timezone": job.timezone,
        "tags": job.tags,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "next_run_at": job.next_run_at,
        "last_run_at": job.last_run_at,
        "consecutive_failures": job.consecutive_failures,
        "priority": job.priority,
        "schedule_kind": job.schedule_kind.as_str(),
    })
}

/// Serialize a JobRun to JSON
fn run_to_json(run: &JobRun) -> Value {
    json!({
        "id": run.id,
        "job_id": run.job_id,
        "status": format!("{}", run.status),
        "started_at": run.started_at,
        "ended_at": run.ended_at,
        "duration_ms": run.duration_ms,
        "error": run.error,
        "response": run.response,
    })
}

// ============================================================================
// Real handlers (backed by CronService)
// ============================================================================

/// Handle cron.list RPC request (real)
///
/// Returns a list of all configured cron jobs from the CronService.
pub async fn handle_list(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let jobs_json: Vec<Value> = jobs.iter().map(job_to_json).collect();
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
///
/// Returns details of a single cron job by ID from the CronService.
pub async fn handle_get(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    match service.get_job(&job_id).await {
        Ok(job) => JsonRpcResponse::success(request.id, json!({ "job": job_to_json(&job) })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get job: {}", e),
        ),
    }
}

/// Handle cron.create RPC request (real)
///
/// Creates a new cron job via the CronService.
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

    let schedule = match params.get("schedule").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing schedule");
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

    let job = CronJob::new(name, schedule, agent_id, prompt);

    let service = cron.lock().await;
    match service.add_job(job.clone()).await {
        Ok(job_id) => {
            // Re-fetch to get the full job state (including computed next_run_at)
            match service.get_job(&job_id).await {
                Ok(created_job) => JsonRpcResponse::success(
                    request.id,
                    json!({ "job": job_to_json(&created_job) }),
                ),
                Err(_) => {
                    // Fallback: return the job as constructed
                    JsonRpcResponse::success(request.id, json!({ "job": job_to_json(&job) }))
                }
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create job: {}", e),
        ),
    }
}

/// Handle cron.update RPC request (real)
///
/// Updates an existing cron job by ID via the CronService.
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

    let service = cron.lock().await;

    // Fetch existing job
    let mut job = match service.get_job(&job_id).await {
        Ok(j) => j,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get job for update: {}", e),
            );
        }
    };

    // Apply partial updates
    if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
        job.name = name.to_string();
    }
    if let Some(schedule) = params.get("schedule").and_then(|v| v.as_str()) {
        job.schedule = schedule.to_string();
    }
    if let Some(agent_id) = params.get("agent_id").and_then(|v| v.as_str()) {
        job.agent_id = agent_id.to_string();
    }
    if let Some(prompt) = params.get("prompt").and_then(|v| v.as_str()) {
        job.prompt = prompt.to_string();
    }
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        job.enabled = enabled;
    }

    match service.update_job(job).await {
        Ok(()) => {
            // Re-fetch updated job
            match service.get_job(&job_id).await {
                Ok(updated) => JsonRpcResponse::success(
                    request.id,
                    json!({ "job": job_to_json(&updated) }),
                ),
                Err(_) => JsonRpcResponse::success(
                    request.id,
                    json!({ "job": { "id": job_id, "updated": true } }),
                ),
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update job: {}", e),
        ),
    }
}

/// Handle cron.delete RPC request (real)
///
/// Deletes a cron job by ID via the CronService.
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
///
/// Returns the status of the cron service (job count).
pub async fn handle_status(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => JsonRpcResponse::success(
            request.id,
            json!({
                "running": true,
                "job_count": jobs.len(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get status: {}", e),
        ),
    }
}

/// Handle cron.run RPC request (real)
///
/// Manually triggers a cron job by ID. Drops the mutex lock before awaiting
/// the executor future to avoid holding the lock across an await point.
pub async fn handle_run(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    // Scope the lock: fetch job + executor, then drop lock before awaiting
    let (job, executor) = {
        let service = cron.lock().await;
        let job = match service.get_job(&job_id).await {
            Ok(j) => j,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to get job: {}", e),
                );
            }
        };
        let executor = match service.executor_ref() {
            Some(e) => e.clone(),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    "No executor configured for cron service",
                );
            }
        };
        (job, executor)
    };
    // Lock is dropped here

    // Execute the job outside the lock
    match executor(job.id.clone(), job.agent_id.clone(), job.prompt.clone()).await {
        Ok(response) => JsonRpcResponse::success(
            request.id,
            json!({
                "triggered": job_id,
                "status": "completed",
                "response": response,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Job execution failed: {}", e),
        ),
    }
}

/// Handle cron.runs RPC request (real)
///
/// Returns the execution history for a cron job.
pub async fn handle_runs(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let limit = match &request.params {
        Some(Value::Object(map)) => map
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize,
        _ => 50,
    };

    let service = cron.lock().await;
    match service.get_job_runs(&job_id, limit).await {
        Ok(runs) => {
            let runs_json: Vec<Value> = runs.iter().map(run_to_json).collect();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "job_id": job_id,
                    "runs": runs_json,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get runs: {}", e),
        ),
    }
}

/// Handle cron.toggle RPC request (real)
///
/// Enables or disables a cron job by ID via the CronService.
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

    let enabled = params
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let service = cron.lock().await;
    let result = if enabled {
        service.enable_job(&job_id).await
    } else {
        service.disable_job(&job_id).await
    };

    match result {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "job_id": job_id,
                "enabled": enabled,
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
///
/// Returns an empty list of cron jobs.
pub async fn handle_list_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "jobs": []
        }),
    )
}

/// Handle cron.get RPC request (stub)
///
/// Returns a placeholder cron job by ID.
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
                "schedule": "",
                "enabled": false,
                "created_at": null,
                "updated_at": null
            }
        }),
    )
}

/// Handle cron.create RPC request (stub)
///
/// Creates a fake cron job and returns it with a generated ID.
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
    let schedule = params
        .get("schedule")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    JsonRpcResponse::success(
        request.id,
        json!({
            "job": {
                "id": id,
                "name": name,
                "schedule": schedule,
                "enabled": true,
                "created_at": now,
                "updated_at": now
            }
        }),
    )
}

/// Handle cron.update RPC request (stub)
///
/// Returns a placeholder update confirmation.
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
        json!({
            "job": {
                "id": job_id,
                "updated": true
            }
        }),
    )
}

/// Handle cron.delete RPC request (stub)
///
/// Returns a placeholder delete confirmation.
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

    JsonRpcResponse::success(
        request.id,
        json!({
            "deleted": job_id
        }),
    )
}

/// Handle cron.status RPC request (stub)
///
/// Returns a placeholder cron service status.
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
///
/// Returns a placeholder job trigger confirmation.
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
///
/// Returns an empty execution history for a cron job.
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
///
/// Returns a placeholder toggle confirmation.
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
            Some(json!({ "job_id": "daily-backup", "schedule": "0 1 * * *" })),
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
