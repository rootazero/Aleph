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
//!
//! ## The operator's half is ratified, and audited (human ruling, 2026-08-07)
//!
//! Admin-gating `trace.list`/`trace.get` decides WHO may read; it says nothing
//! about whether that read is recorded. It was not: an operator could read any
//! member's full transcript and leave no trace of having done so, while P1's
//! own acceptance test asserts elsewhere that "the operator is not exempt from
//! session ownership". Both statements were about different things and read as
//! a contradiction.
//!
//! The ruling keeps the capability — an operator debugging a member's run is
//! the reason these methods exist — and adds the missing half:
//! [`AuditEventType::ScopedContentRead`](crate::security::audit::AuditEventType::ScopedContentRead)
//! is emitted from both handlers when the caller could NOT have reached that
//! content through the ordinary owner-scoped surface. Reading your OWN trace
//! is not an event, so a single-user box records nothing at all — that is what
//! keeps the log readable enough to be worth having. See
//! [`caller_could_reach`] for why the predicate is `session_visible_to` and
//! not a bare owner comparison.

use std::collections::{HashMap, HashSet};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::resilience::StateDatabase;
use crate::security::audit::{AuditEntry, SecurityAuditLog};
use crate::sync_primitives::Arc;
use aleph_protocol::{
    AgentTraceListCursor, AgentTraceListPage, AgentTraceListRow, AgentTraceReplay,
    AgentTraceReplayEntry, AgentTraceTaskSummary,
};
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
/// answered from the trace store itself, and the run→session index
/// `event_visibility` keeps is live-only — it evicts on `RunComplete`, and
/// this method reads history.
///
/// This doc used to add that `agent_tasks` "does not close the gap either:
/// root-agent runs — exactly the ones the Panel replays — have no task row at
/// all". That is false for any run that HAS traces to replay, and the false
/// premise is a large part of why this family's ownership looked unanswerable
/// for as long as it did: `execute.rs` switches trace persistence on ONLY when
/// the `agent_tasks` insert succeeded (`trace_task_persisted.then(...)`), and
/// nothing anywhere deletes a task row. The 2026-08-07 audit walks that hop
/// ([`session_of_run`]) precisely because it is there.
///
/// This method still names the session rather than walking the hop, for the
/// two reasons that survive the correction: it is an ADDRESSED surface, so it
/// owes the byte-identical `not_found` a wrong key produces; and holding the
/// session is what lets it intersect the requested runs with that session's
/// own. Two checks, in order:
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
        .get_history(&session_key, Some(MAX_HISTORY_SCAN))
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

/// The session a persisted run belongs to, or `None` when that cannot be
/// established.
///
/// A `task_traces` row records only its `task_id`; the run's session key is
/// one hop away, on the `agent_tasks` row
/// (`execution_engine::persistence::persist_run_task_started` writes
/// `request.session_key.to_key_string()` there, and trace persistence is
/// switched on by that same insert succeeding — `execute.rs`'s
/// `trace_task_persisted.then(...)` — so a persisted trace normally HAS a
/// row). "Normally" is not "always", which is why every caller of this treats
/// `None` as unattributable rather than as harmless.
async fn session_of_run(db: &StateDatabase, task_id: &str) -> Option<String> {
    match db.get_agent_task(task_id).await {
        Ok(Some(task)) => Some(task.parent_session_id),
        _ => None,
    }
}

/// Whether `caller` could have reached this session's content through the
/// ordinary owner-scoped surface — i.e. whether this read needed the admin
/// gate at all.
///
/// The ruling's words are "the session's `effective_owner`", but a bare
/// `effective_owner(&meta) == caller` is the exact comparison
/// `visibility`'s module doc forbids handlers from writing, and after P2 it is
/// also wrong: a project room's `owner_user_id` is its CREATOR, so every other
/// member of a room would be recorded as reading somebody else's transcript
/// while `trace.by_runs` hands them the same bytes without a gate. The
/// question the audit actually asks is "did the admin gate let this through",
/// and the predicate for that is the one `handle_by_runs` denies with:
/// [`visibility::session_visible_to`].
///
/// Fails toward RECORDING: an unattributable read — no task row, a key that
/// no longer parses, a deleted session, a store error — is one we cannot
/// prove was the caller's own, and silence about it would be the same
/// fail-soft-read-as-absence this repo has been bitten by before.
async fn caller_could_reach(
    sessions: &dyn SessionStore,
    session_id: Option<&str>,
    caller: &str,
) -> bool {
    let Some(key) = session_id.and_then(SessionKey::from_key_string) else {
        return false;
    };
    matches!(
        sessions.get_metadata(&key).await,
        Ok(Some(meta)) if visibility::session_visible_to(&meta, caller)
    )
}

#[derive(Debug, Default, Deserialize)]
struct TraceListParams {
    #[serde(default)]
    limit: Option<usize>,
    /// Cursor: return tasks ordered strictly before
    /// `(timestamp, task_id)` in the `(last_timestamp DESC, task_id DESC)`
    /// ordering. `Some(None)` is the start of the list; `None` falls back
    /// to the legacy single-timestamp cursor (`before_timestamp`) for
    /// backward compatibility with older callers.
    #[serde(default)]
    before: Option<Option<TraceCursor>>,
    /// Legacy single-timestamp cursor. Used only when `before` is absent;
    /// the compound `before` supersedes it because the timestamp alone
    /// drops rows that share a second with the previous page.
    #[serde(default)]
    before_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct TraceCursor {
    last_timestamp: i64,
    task_id: String,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

/// Read-only: page through the process-wide index of persisted traces.
///
/// Admin-gated (`method_admin.rs`'s `trace.` prefix) because it enumerates
/// EVERY run in the process, across every user. That capability is ratified;
/// what it now also does is leave a record of itself — one
/// `ScopedContentRead` entry per call when the page names a run the caller
/// could not have reached on their own, never one per row, and nothing at all
/// when every run on the page is theirs (see this module's doc).
pub async fn handle_list(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
    audit: Option<SecurityAuditLog>,
) -> JsonRpcResponse {
    // A params object that does not deserialize is NOT "no params". Every
    // field here is `#[serde(default)]`, so `{}` and a missing `params` both
    // parse; the only way this fails is a field of the wrong TYPE — a cursor
    // in the wrong shape, most likely. `unwrap_or_default()` answered that
    // with `TraceListParams::default()`: no cursor and the 50-row default
    // limit, i.e. page one again, reported as success. A caller paging through
    // an admin enumeration would loop on the first page forever and never be
    // told why. `gateway_trace_replay_rpc::list_cursor_advances_without_overlap`
    // spent the change from a single-timestamp cursor to the compound one
    // inside this hole.
    let params: TraceListParams = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("trace.list: invalid params: {e}"),
                )
            }
        },
        None => TraceListParams::default(),
    };
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

    // Prefer the compound cursor (`before`) over the legacy single-timestamp
    // form (`before_timestamp`). The compound form is required for correctness
    // when tasks share an epoch-second timestamp; the legacy form silently
    // drops such rows. We surface a deprecation note in the response so
    // operators can spot any client still using the old key.
    let cursor = match params.before {
        Some(Some(c)) => Some((c.last_timestamp, c.task_id)),
        Some(None) => None,
        None => {
            if params.before_timestamp.is_some() {
                tracing::warn!(
                    "trace.list: client supplied legacy single-timestamp cursor; \
                     switch to `before: {{ last_timestamp, task_id }}` to avoid \
                     silently dropping rows on timestamp collisions"
                );
            }
            params
                .before_timestamp
                .map(|ts| (ts, "\u{10ffff}".to_string()))
        }
    };
    match db.list_trace_tasks_paged(limit, cursor).await {
        Ok(tasks) => {
            // An unscoped caller (cron / in-process) resolves to `None` here
            // and is not audited, exactly as it is not filtered — the same
            // zero-change arm every predicate in `visibility` opens with.
            if let (Some(log), Some(caller)) = (audit.as_ref(), visibility::visible_owner_filter())
            {
                let mut reach: HashMap<String, bool> = HashMap::new();
                for task in &tasks {
                    let session = session_of_run(&db, &task.task_id).await;
                    // Memoised per SESSION, not per task: a page is usually a
                    // handful of conversations, and the session lookup is the
                    // expensive half (the `agent_tasks` hop is a point query).
                    let reachable = match reach.get(session.as_deref().unwrap_or_default()) {
                        Some(known) => *known,
                        None => {
                            let known =
                                caller_could_reach(sessions.as_ref(), session.as_deref(), &caller)
                                    .await;
                            reach.insert(session.clone().unwrap_or_default(), known);
                            known
                        }
                    };
                    if !reachable {
                        log.log(AuditEntry::scoped_content_read(
                            caller.clone(),
                            session,
                            format!(
                                "trace.list: page of {} includes run {} the caller cannot reach",
                                tasks.len(),
                                task.task_id
                            ),
                        ));
                        break;
                    }
                }
            }

            // Cursor exhaustion: if fewer than `limit` rows returned, there's
            // no next page. Otherwise, the next page starts strictly before
            // the last entry in (last_timestamp DESC, task_id DESC) order.
            // The compound cursor is required: a single-timestamp cursor
            // drops rows whose `last_timestamp` collides with the previous
            // page's last entry (see `list_trace_tasks_paged`).
            let exhausted = tasks.len() < limit;
            let next_cursor = if exhausted {
                None
            } else {
                tasks.last().map(|t| AgentTraceListCursor {
                    last_timestamp: t.last_timestamp,
                    task_id: t.task_id.clone(),
                })
            };
            // CONSTRUCTED from the contract type, never hand-written as
            // `json!`. Parsing against a contract can only prove the response
            // is a SUPERSET of it; constructing from one makes both halves of
            // the mismatch — a missing field and an extra one — unrepresentable.
            // See `AgentTraceListRow`'s doc for the three clients this broke.
            let page = AgentTraceListPage {
                traces: tasks
                    .into_iter()
                    .map(|t| AgentTraceListRow {
                        task_id: t.task_id,
                        // Same defensive word `handle_get` uses when the parent
                        // row is gone; the FK makes that unreachable in a
                        // healthy database.
                        status: t
                            .status
                            .map_or_else(|| "unknown".to_string(), |s| s.to_lowercase()),
                        started_at: t.started_at,
                        last_timestamp: t.last_timestamp,
                        event_count: usize::try_from(t.event_count).unwrap_or(0),
                        prompt_preview: t.prompt_preview.unwrap_or_default(),
                    })
                    .collect(),
                next_cursor,
            };
            match serde_json::to_value(&page) {
                Ok(v) => JsonRpcResponse::success(request.id, v),
                Err(e) => {
                    tracing::error!(error = %e, "trace.list: serialize failed");
                    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to list traces")
                }
            }
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
/// `agent_tasks` table, synthesized from the trace stream when no task row
/// exists. That fallback is defensive, not routine — a persisted trace
/// normally HAS a task row (see [`handle_by_runs`]) — which is why an
/// unattributable read is treated as one worth recording rather than as the
/// common case. A task with no persisted traces is "not found".
///
/// Admin-gated, and the read is RECORDED when the run's session is one the
/// caller could not have reached through `trace.by_runs` — the ratified
/// operator capability plus its accountability half (see this module's doc).
/// The record is emitted only on the path that actually returns bytes: the
/// two early "not found" arms above disclose nothing and are not reads.
pub async fn handle_get(
    request: JsonRpcRequest,
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
    audit: Option<SecurityAuditLog>,
) -> JsonRpcResponse {
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

    let task_row = db.get_agent_task(&task_id).await.ok().flatten();

    // The bytes are about to go out; record the read first, so a serialization
    // failure below cannot be the difference between a disclosed transcript
    // and an unrecorded one.
    if let (Some(log), Some(caller)) = (audit.as_ref(), visibility::visible_owner_filter()) {
        let session = task_row.as_ref().map(|t| t.parent_session_id.clone());
        if !caller_could_reach(sessions.as_ref(), session.as_deref(), &caller).await {
            log.log(AuditEntry::scoped_content_read(
                caller,
                session,
                format!("trace.get: read {} events of run {task_id}", traces.len()),
            ));
        }
    }

    let task = match task_row {
        Some(t) => AgentTraceTaskSummary {
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
        // No task row: synthesize from the trace stream. Defensive — see the
        // doc above for why this is not the routine case it used to claim.
        None => {
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

    /// [`seed_run`]'s sibling for the audit tests: the `agent_tasks` row names
    /// a REAL session key, which is the shape production writes
    /// (`execution_engine::persistence::persist_run_task_started` stores
    /// `request.session_key.to_key_string()`) and the hop
    /// [`session_of_run`] walks. `seed_run` keeps its `"s"` placeholder on
    /// purpose so the older tests still cover the unattributable path.
    async fn seed_run_in(db: &StateDatabase, run_id: &str, key: &SessionKey, texts: &[&str]) {
        db.insert_agent_task(&AgentTask::new(
            run_id,
            key.to_key_string(),
            "coder",
            "x",
            RiskLevel::Low,
        ))
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
        let temp = TempDir::new().unwrap();

        let resp = handle_get(
            req(json!({ "task_id": "run-a" })),
            db,
            session_store(&temp),
            None,
        )
        .await;

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
        let temp = TempDir::new().unwrap();
        let resp = handle_get(req(json!({})), db, session_store(&temp), None).await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().message, "Missing task_id");
    }

    #[tokio::test]
    async fn get_unknown_task_is_not_found() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let resp = handle_get(
            req(json!({ "task_id": "nope" })),
            db,
            session_store(&temp),
            None,
        )
        .await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().message, "Trace not found");
    }

    // ───────────── RULING 2 (2026-08-07): ratified, and audited ─────────────

    /// The whole ruling in one test: the operator STILL GETS THE BYTES (this
    /// is a ratification, not a new denial — assert the transcript text is in
    /// the response, or the test would also pass against a handler that
    /// refused), and the read now names itself in the audit log — who read,
    /// whose session, which run.
    #[tokio::test]
    async fn trace_get_of_another_users_run_is_served_and_audited() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let alice_key = SessionKey::main("conv-alice");
        seed_session(&sessions, &alice_key, "u-alice", &["run-a"]).await;
        seed_run_in(&db, "run-a", &alice_key, &["alice's secret prompt"]).await;

        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(8);
        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(
                    req(json!({ "task_id": "run-a" })),
                    db,
                    sessions,
                    Some(log.clone()),
                ),
            )
            .await;

        let result = resp
            .result
            .expect("the operator read is ratified, not denied");
        assert!(
            serde_json::to_string(&result)
                .unwrap()
                .contains("alice's secret prompt"),
            "the ruling KEEPS the capability — if this ever stops serving bytes \
             the audit half is pointless: {result}"
        );

        let entry = rx.try_recv().expect("the read must leave a record");
        assert_eq!(
            entry.event_type,
            crate::security::audit::AuditEventType::ScopedContentRead
        );
        assert_eq!(
            entry.actor_user.as_deref(),
            Some("u-bob"),
            "the record must name WHO read — the question the log could not \
             answer before this variant existed"
        );
        assert_eq!(
            entry.session_id.as_deref(),
            Some(alice_key.to_key_string().as_str()),
            "…and WHOSE session was read"
        );
        assert!(entry.detail.contains("run-a"), "{}", entry.detail);
        assert!(
            !entry.detail.contains("alice's secret prompt"),
            "the audit record must name the read, never carry its content: {}",
            entry.detail
        );
        assert!(rx.try_recv().is_err(), "exactly one entry per read");
    }

    /// The clause that keeps the log worth reading: your own transcript is not
    /// an audit event. A single-user box is entirely this case, so getting it
    /// wrong would drown the table in records of the owner reading himself.
    #[tokio::test]
    async fn trace_get_of_your_own_run_is_not_audited() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-alice");
        seed_session(&sessions, &key, "u-alice", &["run-a"]).await;
        seed_run_in(&db, "run-a", &key, &["a0"]).await;

        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(8);
        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_get(
                    req(json!({ "task_id": "run-a" })),
                    db,
                    sessions,
                    Some(log.clone()),
                ),
            )
            .await;

        assert!(resp.result.is_some(), "alice must still get her own trace");
        assert!(
            rx.try_recv().is_err(),
            "reading your own trace is not an audit event"
        );
    }

    /// A room member is not the room session's `owner_user_id` — its creator
    /// is — so an owner-equality predicate would file every OTHER member's
    /// perfectly ordinary read as a cross-user disclosure. The predicate is
    /// `session_visible_to`, the same one `trace.by_runs` denies with, so what
    /// gets recorded is "the admin gate was needed", not "you are not the
    /// creator".
    #[tokio::test]
    async fn a_room_members_read_of_the_rooms_run_is_not_audited() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let project_id = format!("p-{}", uuid::Uuid::new_v4().simple());
        crate::projects::roster::publish(crate::projects::roster::RosterSnapshot::from_pairs([
            (project_id.clone(), "u-alice".to_string()),
            (project_id.clone(), "u-bob".to_string()),
        ]));

        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-room");
        // Created BY alice, scoped to the room: `owner_user_id` is alice.
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".to_string(),
                scope: crate::scope::ScopeId::Project(project_id.clone()),
            }),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();
        seed_run_in(&db, "run-room", &key, &["shared room turn"]).await;

        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(8);
        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(
                    req(json!({ "task_id": "run-room" })),
                    db,
                    sessions,
                    Some(log.clone()),
                ),
            )
            .await;

        assert!(resp.result.is_some(), "bob is on the roster");
        assert!(
            rx.try_recv().is_err(),
            "a roster member reading the room's own run is not a cross-user read"
        );
    }

    /// The `trace.list` wire shape must be EXACTLY `AgentTraceListPage` — no
    /// missing key, and no extra one.
    ///
    /// ⚠️ Both halves are load-bearing and only one of them is what a
    /// "does it parse?" test proves. `serde` ignores unknown keys, so parsing a
    /// live response into the contract type can only ever show the response is
    /// a SUPERSET of the contract; it is structurally blind to over-sending,
    /// which is how `workspace.get` shipped four fields with neither a writer
    /// nor a reader. So the expected key set here is DERIVED from the contract
    /// type itself and compared for EQUALITY — a field added to the response by
    /// hand reds this test, and so does a field renamed out from under a
    /// client.
    ///
    /// The last two assertions are the other half of the 2026-08-29 fix: the
    /// three facts `status` / `started_at` / `prompt_preview` are not padding,
    /// they are read from the `agent_tasks` parent through the LEFT JOIN. If
    /// that join is ever dropped, every row degrades to `"unknown"` + `""` —
    /// a column of dashes that reads as "no value yet" rather than as a broken
    /// query, which is precisely the failure mode this family keeps repeating.
    #[tokio::test]
    async fn the_trace_list_response_has_exactly_the_contract_keys() {
        use std::collections::BTreeSet;

        fn keys(v: &Value) -> BTreeSet<String> {
            v.as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
        }

        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-contract");
        seed_session(&sessions, &key, "u-alice", &["run-contract"]).await;
        seed_run_in(&db, "run-contract", &key, &["t0", "t1"]).await;

        let result = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_list(req(json!({})), db, sessions, None),
            )
            .await
            .result
            .expect("success");

        // (1) It parses — the client half.
        let page: AgentTraceListPage =
            serde_json::from_value(result.clone()).expect("the response IS the contract type");
        assert_eq!(page.traces.len(), 1, "self-guard: one seeded run");

        // (2) It over-sends nothing — the half `from_value` cannot see.
        //     Expected keys derived from the type, never listed by hand.
        let expected = serde_json::to_value(&page).expect("re-serialize");
        assert_eq!(
            keys(&result),
            keys(&expected),
            "envelope keys must equal AgentTraceListPage's exactly"
        );
        assert_eq!(
            keys(&result["traces"][0]),
            keys(&expected["traces"][0]),
            "row keys must equal AgentTraceListRow's exactly"
        );

        // (3) The parent-row facts really arrive.
        let row = &page.traces[0];
        assert_eq!(row.event_count, 2);
        assert_ne!(
            row.status, "unknown",
            "the agent_tasks LEFT JOIN must reach the parent row — `unknown` \
             here means every STATUS cell in `aleph trace list` is a dash"
        );
        assert!(
            !row.prompt_preview.is_empty(),
            "prompt_preview must come from agent_tasks.task_prompt"
        );
    }

    /// `trace.list` enumerates every run in the process. One record per CALL
    /// naming the first run the caller cannot reach — not one per row, which
    /// would make a 50-row page 50 rows of log.
    #[tokio::test]
    async fn trace_list_records_one_entry_when_the_page_names_a_foreign_run() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let alice_key = SessionKey::main("conv-alice");
        let bob_key = SessionKey::main("conv-bob");
        seed_session(&sessions, &alice_key, "u-alice", &[]).await;
        seed_session(&sessions, &bob_key, "u-bob", &[]).await;
        seed_run_in(&db, "run-alice", &alice_key, &["a0"]).await;
        seed_run_in(&db, "run-bob", &bob_key, &["b0"]).await;

        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(8);
        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list(req(json!({})), db, sessions, Some(log.clone())),
            )
            .await;

        let traces = resp.result.expect("success")["traces"]
            .as_array()
            .expect("traces array")
            .len();
        assert_eq!(traces, 2, "the enumeration itself is unchanged — ratified");

        let entry = rx
            .try_recv()
            .expect("enumerating a foreign run is recorded");
        assert_eq!(
            entry.event_type,
            crate::security::audit::AuditEventType::ScopedContentRead
        );
        assert_eq!(entry.actor_user.as_deref(), Some("u-bob"));
        assert!(entry.detail.contains("trace.list"), "{}", entry.detail);
        assert!(
            rx.try_recv().is_err(),
            "one entry per call, not one per row"
        );
    }

    /// …and the page that is entirely your own records nothing, which is the
    /// single-user box's whole experience of this feature.
    #[tokio::test]
    async fn trace_list_of_only_your_own_runs_is_not_audited() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let bob_key = SessionKey::main("conv-bob");
        seed_session(&sessions, &bob_key, "u-bob", &[]).await;
        seed_run_in(&db, "run-bob-1", &bob_key, &["b0"]).await;
        seed_run_in(&db, "run-bob-2", &bob_key, &["b1"]).await;

        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(8);
        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list(req(json!({})), db, sessions, Some(log.clone())),
            )
            .await;

        assert!(resp.result.is_some());
        assert!(
            rx.try_recv().is_err(),
            "a page of your own runs is not a cross-user read"
        );
    }
}
