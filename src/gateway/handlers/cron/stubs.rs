//! Stub cron handlers — stateless, used in HandlerRegistry::new()
//! before the real `CronService` is wired in.

use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

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
