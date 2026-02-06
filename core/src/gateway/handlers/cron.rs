//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, status, run.

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Handle cron.list RPC request
///
/// Returns a list of all configured cron jobs.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronManager when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "jobs": []
        }),
    )
}

/// Handle cron.status RPC request
///
/// Returns the status of the cron service.
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronManager when available
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

    // TODO: Integrate with actual CronManager when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "triggered": job_id,
            "status": "queued"
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
}
