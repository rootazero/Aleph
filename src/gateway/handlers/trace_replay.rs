use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use serde_json::{json, Value};

pub async fn handle_list(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
) -> JsonRpcResponse {
    match db.list_trace_tasks().await {
        Ok(tasks) => {
            let traces: Vec<Value> = tasks
                .into_iter()
                .map(|t| {
                    json!({
                        "task_id": t.task_id,
                        "event_count": t.event_count,
                        "last_timestamp": t.last_timestamp
                    })
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "traces": traces }))
        }
        Err(e) => {
            tracing::error!("Failed to list traces: {}", e);
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to list traces")
        }
    }
}

pub async fn handle_get(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
) -> JsonRpcResponse {
    let trace_id = match request
        .params
        .as_ref()
        .and_then(|p| p.get("trace_id"))
        .and_then(|v| v.as_i64())
    {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing trace_id");
        }
    };

    match db.get_trace_by_id(trace_id).await {
        Ok(Some(trace)) => JsonRpcResponse::success(
            request.id,
            json!({
                "trace": {
                    "id": trace.id,
                    "task_id": trace.task_id,
                    "step_index": trace.step_index,
                    "event": trace.event,
                    "timestamp": trace.timestamp
                }
            }),
        ),
        Ok(None) => {
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Trace not found")
        }
        Err(e) => {
            tracing::error!("Failed to get trace: {}", e);
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to get trace")
        }
    }
}

pub async fn handle_list_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let _ = request
        .params
        .as_ref()
        .and_then(|p| p.get("session_id"))
        .and_then(|v| v.as_str());
    JsonRpcResponse::success(request.id, json!({ "traces": [] }))
}

pub async fn handle_get_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let trace_id = match request
        .params
        .as_ref()
        .and_then(|p| p.get("trace_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing trace_id");
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "trace": {
                "id": trace_id,
                "task_id": "",
                "step_index": 0,
                "event": null,
                "timestamp": null
            }
        }),
    )
}
