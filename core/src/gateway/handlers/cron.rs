//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, get, create, update, delete,
//! status, run, runs, toggle.

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Handle cron.list RPC request
///
/// Returns a list of all configured cron jobs.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "jobs": []
        }),
    )
}

/// Handle cron.get RPC request
///
/// Returns details of a single cron job by ID.
pub async fn handle_get(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
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

/// Handle cron.create RPC request
///
/// Creates a new cron job and returns it with a generated ID.
pub async fn handle_create(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
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

/// Handle cron.update RPC request
///
/// Updates an existing cron job by ID.
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
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

/// Handle cron.delete RPC request
///
/// Deletes a cron job by ID.
pub async fn handle_delete(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "deleted": job_id
        }),
    )
}

/// Handle cron.status RPC request
///
/// Returns the status of the cron service.
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "job_count": 0,
            "last_tick": null
        }),
    )
}

/// Handle cron.run RPC request
///
/// Manually triggers a cron job by ID.
pub async fn handle_run(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "triggered": job_id,
            "status": "queued"
        }),
    )
}

/// Handle cron.runs RPC request
///
/// Returns the execution history for a cron job.
pub async fn handle_runs(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "job_id": job_id,
            "runs": []
        }),
    )
}

/// Handle cron.toggle RPC request
///
/// Enables or disables a cron job by ID.
pub async fn handle_toggle(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Integrate with actual CronService when available
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
    async fn test_handle_list() {
        let request = JsonRpcRequest::with_id("cron.list", None, json!(1));
        let response = handle_list(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get() {
        let request = JsonRpcRequest::new(
            "cron.get",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_get(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.get", None, json!(1));
        let response = handle_get(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create() {
        let request = JsonRpcRequest::new(
            "cron.create",
            Some(json!({ "name": "daily-backup", "schedule": "0 0 * * *" })),
            Some(json!(1)),
        );
        let response = handle_create(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_create_missing_params() {
        let request = JsonRpcRequest::with_id("cron.create", None, json!(1));
        let response = handle_create(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_update() {
        let request = JsonRpcRequest::new(
            "cron.update",
            Some(json!({ "job_id": "daily-backup", "schedule": "0 1 * * *" })),
            Some(json!(1)),
        );
        let response = handle_update(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_update_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.update", None, json!(1));
        let response = handle_update(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_delete() {
        let request = JsonRpcRequest::new(
            "cron.delete",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_delete(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_delete_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.delete", None, json!(1));
        let response = handle_delete(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_status() {
        let request = JsonRpcRequest::with_id("cron.status", None, json!(1));
        let response = handle_status(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run() {
        let request = JsonRpcRequest::new(
            "cron.run",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_run(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.run", None, json!(1));
        let response = handle_run(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_runs() {
        let request = JsonRpcRequest::new(
            "cron.runs",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_runs(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_runs_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.runs", None, json!(1));
        let response = handle_runs(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_toggle() {
        let request = JsonRpcRequest::new(
            "cron.toggle",
            Some(json!({ "job_id": "daily-backup", "enabled": false })),
            Some(json!(1)),
        );
        let response = handle_toggle(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_toggle_missing_params() {
        let request = JsonRpcRequest::with_id("cron.toggle", None, json!(1));
        let response = handle_toggle(request).await;
        assert!(response.is_error());
    }
}
