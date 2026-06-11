use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use aleph_protocol::{AgentTraceReplay, AgentTraceReplayEntry, AgentTraceTaskSummary};
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
/// `run_id` (= `task_id`), grouped by run, ordered by `step_index`. Unknown or
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
                    .map_or(Value::Null, |t| json!(t.last_timestamp))
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

/// Read-only: return the full persisted trace replay for one `task_id`
/// (= `run_id`), as the `AgentTraceReplay` envelope the panel deserializes:
/// `{ task: AgentTraceTaskSummary, traces: [{ step, event }] }`. The traces
/// come from the `task_traces` observability table; the task summary from the
/// `agent_tasks` table (synthesized from the trace stream when no task row
/// exists, e.g. root-agent runs). A task with no persisted traces is "not
/// found".
pub async fn handle_get(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
    let task_id = match request
        .params
        .as_ref()
        .and_then(|p| p.get("task_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id");
        }
    };

    let traces = match db.get_traces_by_task(&task_id).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(task_id = %task_id, error = %e, "trace.get: load failed");
            return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to get trace");
        }
    };

    if traces.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Trace not found");
    }

    let entries: Vec<AgentTraceReplayEntry> = traces
        .iter()
        .map(|t| AgentTraceReplayEntry {
            step: u64::from(t.step_index),
            event: t.event.clone(),
        })
        .collect();

    // Derive the last event's serde tag ("kind") for the summary badge.
    let last_event_kind = traces
        .last()
        .and_then(|t| serde_json::to_value(&t.event).ok())
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from));

    let task = match db.get_agent_task(&task_id).await {
        Ok(Some(t)) => AgentTraceTaskSummary {
            task_id: t.id,
            session_id: t.parent_session_id,
            agent_id: t.agent_id,
            status: format!("{:?}", t.status).to_lowercase(),
            prompt_preview: t.task_prompt.chars().take(200).collect(),
            created_at: t.created_at.max(0) as u64,
            updated_at: t.updated_at.max(0) as u64,
            started_at: t.started_at.map(|v| v.max(0) as u64),
            completed_at: t.completed_at.map(|v| v.max(0) as u64),
            trace_count: traces.len(),
            last_event_kind,
        },
        // No task row (e.g. root-agent run): synthesize from the trace stream.
        _ => {
            let first_ts = traces
                .first()
                .map_or(0, |t| t.timestamp.max(0) as u64);
            let last_ts = traces
                .last()
                .map_or(0, |t| t.timestamp.max(0) as u64);
            AgentTraceTaskSummary {
                task_id: task_id.clone(),
                session_id: String::new(),
                agent_id: String::new(),
                status: "unknown".to_string(),
                prompt_preview: String::new(),
                created_at: first_ts,
                updated_at: last_ts,
                started_at: None,
                completed_at: None,
                trace_count: traces.len(),
                last_event_kind,
            }
        }
    };

    let replay = AgentTraceReplay {
        task,
        traces: entries,
    };
    match serde_json::to_value(&replay) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => {
            tracing::error!(task_id = %task_id, error = %e, "trace.get: serialize failed");
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

    #[tokio::test]
    async fn get_returns_replay_envelope_for_task_id() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-a", &["a0", "a1", "a2"]).await;

        let resp = handle_get(req(json!({ "task_id": "run-a" })), db).await;

        let result = resp.result.expect("success");
        // The panel deserializes the whole AgentTraceReplay envelope.
        let replay: AgentTraceReplay =
            serde_json::from_value(result).expect("AgentTraceReplay shape");
        assert_eq!(replay.task.task_id, "run-a");
        assert_eq!(replay.task.agent_id, "coder");
        assert_eq!(replay.task.trace_count, 3);
        assert_eq!(replay.traces.len(), 3);
        assert_eq!(replay.traces[0].step, 0);
    }

    #[tokio::test]
    async fn get_missing_task_id_is_invalid_params() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let resp = handle_get(req(json!({})), db).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().message, "Missing task_id");
    }

    #[tokio::test]
    async fn get_unknown_task_is_not_found() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let resp = handle_get(req(json!({ "task_id": "nope" })), db).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().message, "Trace not found");
    }
}
