//! Replay of persisted agent traces (`task_traces`).
//!
//! ## P1 visibility (final-review finding C2)
//!
//! A persisted trace is a full transcript: prompts, tool inputs and tool
//! outputs for a run. Nothing here is owner-scoped by construction — a trace
//! row carries a `task_id` (= `run_id`) and nothing else, no session, no
//! owner — so the three methods in this file were, between them, an
//! enumeration oracle (`trace.list` returns every `task_id` in the process)
//! feeding two arbitrary-id readers (`trace.get`, `trace.by_runs`).
//!
//! The family is split by whether a member surface depends on it:
//!
//! - `trace.list` / `trace.get` are **admin-gated** (`method_admin.rs`'s
//!   `trace.` prefix). Their only callers are the operator debugging
//!   surfaces `aleph trace list|get` (CLI) and the TUI's `/trace` command;
//!   the Panel calls neither. Gating `trace.list` is what removes the
//!   enumeration oracle, which is the load-bearing half — the remaining
//!   `run_id`s are uuid4 and are not disclosed to a non-owner by any other
//!   surface (run-correlated events are owner-filtered by
//!   `event_visibility`, and `chat.history`/`sessions.history` are
//!   KeyChecked).
//! - `trace.by_runs` is the Panel's session-hydration path — it runs on every
//!   session open to replay tool calls into the transcript — so it is carved
//!   back open for members and owner-scoped HERE instead, see
//!   [`handle_by_runs`].

use std::collections::HashSet;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use aleph_protocol::{AgentTraceReplay, AgentTraceReplayEntry, AgentTraceTaskSummary};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
struct TraceByRunsParams {
    /// The session these runs belong to. Required — see [`handle_by_runs`].
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    run_ids: Vec<String>,
}

/// Max distinct runs accepted per call (a chat session has a handful).
const MAX_RUNS: usize = 200;

/// How far back through a session's own transcript to look when deciding
/// which `run_id`s belong to it.
///
/// The one caller (`chat_sidebar.rs::hydrate_session_history`) derives its
/// `run_ids` from a 50-message history page, so this window covers it an
/// order of magnitude over. A run older than the window reads as trace-less
/// (an empty array) — the SAME shape an unknown run has always produced, and
/// one the Panel already renders correctly as a plain bubble.
const MAX_HISTORY_SCAN: usize = 500;

/// Read-only: return the persisted agent-trace event stream for each given
/// `run_id` (= `task_id`), grouped by run, ordered by `step_index`. Unknown,
/// trace-less, and not-part-of-this-session runs all yield an empty array
/// (never an error). Reads the `task_traces` observability table only — never
/// the memory store.
///
/// ## Why this takes a `session_key`
///
/// A trace row records only its `task_id`, so "who owns this run" cannot be
/// answered from the trace store, and the `agent_tasks` table does not close
/// the gap either: root-agent runs — exactly the ones the Panel replays —
/// have no task row at all (see [`handle_get`], which synthesizes a summary
/// for them). The run→session index `event_visibility` keeps is live-only; it
/// evicts on `RunComplete`, and this method reads history.
///
/// So the caller names the session instead, and the session is the thing
/// whose ownership IS recorded. Two checks, in order:
///
/// 1. the addressed session is KeyChecked exactly like every other addressed
///    surface (`visibility::session_visible`, denying with
///    `visibility::not_found_response` — byte-identical to a missing key);
/// 2. the requested `run_id`s are intersected with the runs that session's
///    own transcript actually attributes to itself, so proving ownership of
///    one session does not become a licence to read every run in the process.
///
/// A requested run outside that set is not an error and is not distinguished
/// from a run with no traces — both are `[]`, so there is no oracle for
/// "this run exists but is someone else's".
pub async fn handle_by_runs(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: TraceByRunsParams = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(_) => {
                return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params");
            }
        },
        None => TraceByRunsParams::default(),
    };

    // A malformed/absent key is a validation error, not an existence
    // question — the same split `clarification.resolve` already makes.
    let Some(key_str) = params.session_key.as_deref().filter(|s| !s.is_empty()) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
    };
    let Some(session_key) = SessionKey::from_key_string(key_str) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key");
    };

    match sessions.get_metadata(&session_key).await {
        Ok(Some(meta)) if visibility::session_visible(&meta) => {}
        // Foreign owner, missing row, and store error all produce the same
        // response (GC 3 fail-closed, GC 4 no oracle).
        _ => return visibility::not_found_response(request.id),
    }

    let owned_runs: HashSet<String> = match sessions
        .get_history_before(&session_key, Some(MAX_HISTORY_SCAN), None)
        .await
    {
        Ok(messages) => messages
            .into_iter()
            .filter_map(|m| {
                m.metadata
                    .as_ref()?
                    .get("run_id")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect(),
        Err(e) => {
            // Fail closed: without the session's own run list we cannot say a
            // requested run belongs to it.
            tracing::warn!(session_key = %key_str, error = %e, "trace.by_runs: history load failed");
            HashSet::new()
        }
    };

    let mut runs = serde_json::Map::new();
    for run_id in params.run_ids.into_iter().take(MAX_RUNS) {
        let events: Vec<Value> = if owned_runs.contains(&run_id) {
            match db.get_traces_by_task(&run_id).await {
                Ok(traces) => traces
                    .into_iter()
                    .map(|t| serde_json::to_value(&t.event).unwrap_or(Value::Null))
                    .collect(),
                Err(e) => {
                    tracing::warn!(run_id = %run_id, error = %e, "trace.by_runs: load failed");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
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
            let first_ts = traces.first().map_or(0, |t| t.timestamp.max(0) as u64);
            let last_ts = traces.last().map_or(0, |t| t.timestamp.max(0) as u64);
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
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::MessageRecord;
    use crate::resilience::{AgentTask, RiskLevel, TaskTrace};
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
    use tempfile::TempDir;

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

    fn session_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    /// Create `key` owned by `owner` and attribute `run_ids` to it, exactly
    /// the way a real turn does: the run id rides in the message metadata
    /// (`agent_instance::build_message_metadata`), which is where
    /// `chat.history` reads it from too.
    async fn seed_session(
        sessions: &Arc<dyn SessionStore>,
        key: &SessionKey,
        owner: &str,
        run_ids: &[&str],
    ) {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
        for run_id in run_ids {
            sessions
                .append_message(
                    key,
                    MessageRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: "assistant".into(),
                        content: "hi".into(),
                        timestamp: chrono::Utc::now().timestamp(),
                        metadata: Some(json!({ "run_id": run_id })),
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn by_runs_groups_events_per_run_in_step_order() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-a", &["a0", "a1"]).await;
        seed_run(&db, "run-b", &["b0"]).await;

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-owner");
        seed_session(&sessions, &key, "u-alice", &["run-a", "run-b"]).await;

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_by_runs(
                    req(json!({
                        "session_key": key.to_key_string(),
                        "run_ids": ["run-a", "run-b", "run-missing"],
                    })),
                    db,
                    sessions,
                ),
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

    /// The C2 deny: bob addressing alice's session gets the same NOT_FOUND a
    /// missing key produces, and no trace bytes.
    #[tokio::test]
    async fn by_runs_denies_a_foreign_session_with_not_found() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-a", &["alice's secret prompt"]).await;

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-alice");
        seed_session(&sessions, &key, "u-alice", &["run-a"]).await;

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_by_runs(
                    req(json!({
                        "session_key": key.to_key_string(),
                        "run_ids": ["run-a"],
                    })),
                    db.clone(),
                    sessions.clone(),
                ),
            )
            .await;
        assert!(denied.result.is_none(), "must not return trace content");
        assert_eq!(
            denied.error.as_ref().map(|e| e.code),
            Some(crate::gateway::protocol::RESOURCE_NOT_FOUND)
        );

        // Byte-identical to a key that never existed (GC 4, no oracle).
        let missing = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_by_runs(
                    req(json!({
                        "session_key": SessionKey::main("conv-never").to_key_string(),
                        "run_ids": ["run-a"],
                    })),
                    db,
                    sessions,
                ),
            )
            .await;
        assert_eq!(
            serde_json::to_string(&denied).unwrap(),
            serde_json::to_string(&missing).unwrap()
        );
    }

    /// Owning ONE session is not a licence to read every run in the process:
    /// a run that belongs to somebody else's session reads as trace-less,
    /// indistinguishable from a run that has no traces at all.
    #[tokio::test]
    async fn by_runs_will_not_serve_a_run_that_is_not_this_sessions() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-alice", &["alice's secret prompt"]).await;
        seed_run(&db, "run-bob", &["bob's own run"]).await;

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let alice_key = SessionKey::main("conv-a");
        let bob_key = SessionKey::main("conv-b");
        seed_session(&sessions, &alice_key, "u-alice", &["run-alice"]).await;
        seed_session(&sessions, &bob_key, "u-bob", &["run-bob"]).await;

        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_by_runs(
                    req(json!({
                        "session_key": bob_key.to_key_string(),
                        "run_ids": ["run-bob", "run-alice", "run-nonexistent"],
                    })),
                    db,
                    sessions,
                ),
            )
            .await;

        let runs = resp.result.expect("success");
        let runs = runs.get("runs").unwrap();
        assert_eq!(
            runs.get("run-bob").unwrap().as_array().unwrap().len(),
            1,
            "bob must still get his own run — the guard must not be a false positive"
        );
        let foreign = runs.get("run-alice").unwrap().as_array().unwrap();
        assert!(foreign.is_empty(), "alice's run must yield nothing");
        assert_eq!(
            foreign,
            runs.get("run-nonexistent").unwrap().as_array().unwrap(),
            "a foreign run and a nonexistent one must be indistinguishable"
        );
    }

    #[tokio::test]
    async fn by_runs_without_a_session_key_is_invalid_params() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let resp = handle_by_runs(req(json!({ "run_ids": ["run-a"] })), db, sessions).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().message, "Missing session_key");
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
