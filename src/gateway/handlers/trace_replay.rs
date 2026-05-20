use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
struct TraceListParams {
    #[serde(default)]
    limit: Option<usize>,
    /// Cursor: return tasks whose `last_timestamp` is strictly less than this
    /// value. Use the `next_cursor` from the previous response.
    #[serde(default)]
    before_timestamp: Option<i64>,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

pub async fn handle_list(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
    let params: TraceListParams = match request.params.as_ref() {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => TraceListParams::default(),
    };
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

    match db
        .list_trace_tasks_paged(limit, params.before_timestamp)
        .await
    {
        Ok(tasks) => {
            // Cursor exhaustion: if fewer than `limit` rows returned, there's
            // no next page. Otherwise, the next page starts strictly before
            // the smallest last_timestamp in this page.
            let exhausted = tasks.len() < limit;
            let next_cursor = if exhausted {
                Value::Null
            } else {
                tasks
                    .last()
                    .map(|t| json!(t.last_timestamp))
                    .unwrap_or(Value::Null)
            };
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
            JsonRpcResponse::success(
                request.id,
                json!({
                    "traces": traces,
                    "next_cursor": next_cursor,
                }),
            )
        }
        Err(e) => {
            tracing::error!("Failed to list traces: {}", e);
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to list traces")
        }
    }
}

pub async fn handle_get(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
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
        Ok(None) => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Trace not found"),
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
