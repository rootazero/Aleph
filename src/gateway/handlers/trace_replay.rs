use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
struct TraceByRunsParams {
    #[serde(default)]
    run_ids: Vec<String>,
}

/// Max distinct runs accepted per call (a chat session has a handful).
const MAX_RUNS: usize = 200;

/// Read-only: return the persisted agent-trace event stream for each given
/// run_id (= task_id), grouped by run, ordered by step_index. Unknown or
/// trace-less runs yield an empty array (never an error). Reads the
/// `task_traces` observability table only — never the memory store.
pub async fn handle_by_runs(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
    let params: TraceByRunsParams = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(_) => {
                return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params");
            }
        },
        None => TraceByRunsParams::default(),
    };

    let mut runs = serde_json::Map::new();
    for run_id in params.run_ids.into_iter().take(MAX_RUNS) {
        let events: Vec<Value> = match db.get_traces_by_task(&run_id).await {
            Ok(traces) => traces
                .into_iter()
                .map(|t| serde_json::to_value(&t.event).unwrap_or(Value::Null))
                .collect(),
            Err(e) => {
                tracing::warn!(run_id = %run_id, error = %e, "trace.by_runs: load failed");
                Vec::new()
            }
        };
        runs.insert(run_id, Value::Array(events));
    }
    JsonRpcResponse::success(request.id, json!({ "runs": runs }))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::{AgentTask, RiskLevel, TaskTrace};
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "trace.by_runs".into(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    async fn seed_run(db: &StateDatabase, run_id: &str, texts: &[&str]) {
        db.insert_agent_task(&AgentTask::new(run_id, "s", "coder", "x", RiskLevel::Low))
            .await
            .unwrap();
        for (i, t) in texts.iter().enumerate() {
            db.insert_trace(&TaskTrace::new(
                run_id,
                i as u32,
                AgentTraceEvent::TextEmitted {
                    iteration: i,
                    stream: AgentTraceTextKind::Final,
                    text: (*t).to_string(),
                },
            ))
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn by_runs_groups_events_per_run_in_step_order() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-a", &["a0", "a1"]).await;
        seed_run(&db, "run-b", &["b0"]).await;

        let resp = handle_by_runs(
            req(json!({ "run_ids": ["run-a", "run-b", "run-missing"] })),
            db,
        )
        .await;

        let result = resp.result.expect("success");
        let runs = result.get("runs").unwrap();
        assert_eq!(runs.get("run-a").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(runs.get("run-b").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(
            runs.get("run-missing").unwrap().as_array().unwrap().len(),
            0
        );
        let first = &runs.get("run-a").unwrap().as_array().unwrap()[0];
        assert_eq!(first.get("text").unwrap().as_str().unwrap(), "a0");
    }
}
