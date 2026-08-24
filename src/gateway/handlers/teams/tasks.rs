//! Kanban task handlers + chat thread/history + task journal.

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use super::crud::TeamIdParams;
use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStore};
use crate::sync_primitives::Arc;

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::teams::visibility::{gate_task, gate_team};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

// =============================================================================
// Kanban-facing handlers (task list / task update)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListTasksParams {
    pub team_id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
}

/// Handle `teams.list_tasks` — list all `CoordTasks` for a team with optional status/owner filter
pub async fn handle_list_tasks(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_tasks request");

    let params: ListTasksParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &store, &params.team_id).await {
        return resp;
    }

    use crate::agents::swarm::tasks::CoordTaskStatus;
    // Full 10-variant vocabulary via the single-source parser; an unknown
    // string is an explicit error, NOT a silently dropped filter (the old
    // hand-rolled match omitted waiting_review/paused/skipped and returned
    // the entire board as if filtered).
    let status_filter = match params.status.as_deref() {
        None => None,
        Some(s) => match CoordTaskStatus::from_filter_str(s) {
            Some(status) => Some(status),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "Unknown status filter '{s}'. Expected one of: pending, blocked, \
                         in_progress, waiting_review, completed, failed, cancelled, skipped, \
                         paused, unsatisfiable."
                    ),
                );
            }
        },
    };

    let filter = CoordTaskFilter {
        team_id: Some(params.team_id.clone()),
        status: status_filter,
    };

    match coord_store.list_tasks(filter).await {
        Ok(tasks) => JsonRpcResponse::success(request.id, json!({ "tasks": tasks })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list tasks for team '{}': {}", params.team_id, e),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateTaskParams {
    pub task_id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Handle `teams.update_task` — patch a `CoordTask`. Topic emission happens
/// inside the store, so any subscriber sees the change automatically.
pub async fn handle_update_task(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.update_task request");

    let params: UpdateTaskParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskUpdate};

    let status = params.status.as_deref().and_then(|s| match s {
        "pending" => Some(CoordTaskStatus::Pending),
        "in_progress" => Some(CoordTaskStatus::InProgress),
        "completed" => Some(CoordTaskStatus::Completed),
        "failed" => Some(CoordTaskStatus::Failed),
        "cancelled" => Some(CoordTaskStatus::Cancelled),
        // "blocked" is derived, not stored — refuse it
        _ => None,
    });

    if params.status.is_some() && status.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Unknown or unsettable status '{}' (blocked is derived, not stored)",
                params.status.as_deref().unwrap_or("")
            ),
        );
    }

    // Metadata is a shallow merge-patch, not a wholesale replace — the store
    // writes the column verbatim, and a raw panel/LLM patch would wipe the
    // dispatcher control keys (`managed_by`, `max_retries`, `timeout_secs`,
    // review/retry markers), silently dropping the task out of scheduling.
    // Same single-source merge the `task_update` tool uses.
    let metadata = match params.metadata.clone() {
        Some(patch) => match coord_store.get_task(&params.task_id).await {
            Ok(existing) => {
                let existing = existing
                    .map(|t| t.metadata)
                    .unwrap_or(serde_json::Value::Null);
                Some(crate::agents::swarm::tasks::merge_metadata_patch(
                    &existing, patch,
                ))
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!(
                        "Failed to read task '{}' for metadata merge: {e}",
                        params.task_id
                    ),
                )
            }
        },
        None => None,
    };

    let update = CoordTaskUpdate {
        status,
        owner: params.owner.clone(),
        result: params.result.clone(),
        metadata,
    };

    match coord_store.update_task(&params.task_id, update).await {
        Ok(task) => JsonRpcResponse::success(request.id, json!({ "task": task })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update task '{}': {}", params.task_id, e),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskParams {
    pub team_id: String,
    pub subject: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub blocked_by: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Handle `teams.create_task` — create a `CoordTask` on a team's board. Topic
/// emission happens inside the store, so the kanban panel refreshes itself.
pub async fn handle_create_task(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.create_task request");

    let params: CreateTaskParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &store, &params.team_id).await {
        return resp;
    }

    let subject = params.subject.trim();
    if subject.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "subject must not be empty".to_string(),
        );
    }

    use crate::agents::swarm::tasks::{NewCoordTask, Priority};

    // An unknown priority string is a client error rather than a silent
    // default — surface it so the UI can correct the request.
    let priority = match params.priority.as_deref() {
        None => Priority::default(),
        Some(p) => match Priority::from_stored(p) {
            Some(parsed) => parsed,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown priority '{p}' (expected low|normal|high|critical)"),
                )
            }
        },
    };

    let owner = params.owner.filter(|s| !s.trim().is_empty());

    // Panel-created tasks should run on the autonomous dispatcher when an
    // owner is present — otherwise `select_schedulable` filters them out
    // (`is_dispatcher_managed` looks for `metadata.managed_by ==
    // "dispatcher"`) and the kanban silently never advances. Inject the
    // marker unless the caller already supplied one of their own.
    let mut metadata = params
        .metadata
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    if owner.is_some() {
        if let Some(obj) = metadata.as_object_mut() {
            obj.entry("managed_by".to_string())
                .or_insert_with(|| serde_json::Value::String("dispatcher".to_string()));
        }
    }

    let new_task = NewCoordTask {
        team_id: Some(params.team_id.clone()),
        subject: subject.to_string(),
        description: params.description.unwrap_or_default(),
        owner,
        priority,
        blocked_by: params.blocked_by.unwrap_or_default(),
        metadata,
    };

    match coord_store.create_task(new_task).await {
        Ok(task) => JsonRpcResponse::success(request.id, json!({ "task": task })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create task for team '{}': {}", params.team_id, e),
        ),
    }
}

// =============================================================================
// teams.list_task_runs — per-attempt execution history for a task
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TaskIdParams {
    pub task_id: String,
}

/// Handle `teams.list_task_runs` — return all run records for a task, oldest
/// first. The drawer renders this as the "Runs" timeline.
pub async fn handle_list_task_runs(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_runs request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    match coord_store.list_task_runs(&params.task_id).await {
        Ok(runs) => JsonRpcResponse::success(request.id, json!({ "runs": runs })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list runs for task '{}': {}", params.task_id, e),
        ),
    }
}

// =============================================================================
// teams.add_task_comment / list_task_comments — per-task handoff notes
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AddTaskCommentParams {
    pub task_id: String,
    pub author: String,
    pub body: String,
}

/// Handle `teams.add_task_comment` — append a free-text note to a task. Used
/// by workers to leave handoff context and by panel users to annotate state.
pub async fn handle_add_task_comment(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.add_task_comment request");

    let params: AddTaskCommentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    let author = params.author.trim();
    let body = params.body.trim();
    if author.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "author must not be empty".to_string(),
        );
    }
    if body.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "body must not be empty".to_string(),
        );
    }

    match coord_store
        .add_task_comment(&params.task_id, author, body)
        .await
    {
        Ok(comment) => JsonRpcResponse::success(request.id, json!({ "comment": comment })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to add comment to task '{}': {}", params.task_id, e),
        ),
    }
}

/// Handle `teams.list_task_comments` — return all comments for a task,
/// oldest first.
pub async fn handle_list_task_comments(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_comments request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    match coord_store.list_task_comments(&params.task_id).await {
        Ok(comments) => JsonRpcResponse::success(request.id, json!({ "comments": comments })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to list comments for task '{}': {}",
                params.task_id, e
            ),
        ),
    }
}

// =============================================================================
// teams.list_task_events — read team_events filtered to one task_id
// =============================================================================

/// Handle `teams.list_task_events` — return team events whose payload references
/// `task_id`. Walks the team's event log and filters in-memory; cheap for the
/// kanban-scale event volumes in practice (Aleph's logger isn't a firehose).
///
/// Order: oldest first, matching the drawer's "timeline reads top-to-bottom"
/// convention.
pub async fn handle_list_task_events(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    event_store: Arc<dyn crate::teams::events::EventLogStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_events request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    // Look up the task to discover its team_id; without one, no team_events
    // can reference it (team_events.team_id is NOT NULL).
    let task = match coord_store.get_task(&params.task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Task '{}' not found", params.task_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load task '{}': {}", params.task_id, e),
            )
        }
    };
    let Some(team_id) = task.team_id else {
        return JsonRpcResponse::success(request.id, json!({ "events": [] }));
    };

    let events = match event_store.get_events(&team_id, None, None).await {
        Ok(e) => e,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load events for team '{team_id}': {e}"),
            )
        }
    };

    // Filter to events whose payload mentions this task_id. Use string equality
    // to avoid pulling in JSON-pointer machinery — every existing producer
    // emits payload.task_id as a string.
    let filtered: Vec<_> = events
        .into_iter()
        .filter(|e| {
            e.payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .is_some_and(|tid| tid == params.task_id)
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "events": filtered }))
}

// =============================================================================
// teams.task.trace — unified audit timeline (R3 T2)
// =============================================================================
//
// One RPC, one fetch, one wire payload. Aggregates everything needed to
// replay a single task's decision chain:
//
//   { task, runs[], comments[], events[], artifacts[] }
//
// Each sub-list is already individually addressable via list_task_runs /
// list_task_comments / list_task_events / artifact RPCs. The trace
// endpoint is the convenience-store: the panel's Replay view calls this
// once per drawer-open instead of fan-out 4 RPCs.
//
// Notes:
// - Missing optional stores (event_store / artifact_store) yield empty
//   arrays — never an error. The trace is best-effort observability.
// - All four sub-lists are sorted oldest-first so the panel can merge
//   them into a single timeline by `ts` without per-list sort logic.

/// Handle teams.task.trace — return the unified audit timeline for one task.
pub async fn handle_task_trace(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    event_store: Option<Arc<dyn crate::teams::events::EventLogStore>>,
    artifact_store: Option<Arc<dyn crate::teams::artifacts::ArtifactStore>>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.trace request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }

    let task = match coord_store.get_task(&params.task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Task '{}' not found", params.task_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load task '{}': {e}", params.task_id),
            )
        }
    };

    let runs = coord_store
        .list_task_runs(&params.task_id)
        .await
        .unwrap_or_default();

    let comments = coord_store
        .list_task_comments(&params.task_id)
        .await
        .unwrap_or_default();

    let events = if let (Some(store), Some(team_id)) = (&event_store, task.team_id.as_deref()) {
        match store.get_events(team_id, None, None).await {
            Ok(all) => all
                .into_iter()
                .filter(|e| {
                    e.payload
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .is_some_and(|tid| tid == params.task_id)
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let artifacts = if let Some(store) = &artifact_store {
        store
            .get_artifacts_for_task(&params.task_id)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // R3 T3 — fold exit journal into the trace if present. None ⇒ field
    // present but null, so panel can distinguish "no journal yet" from
    // "store unavailable".
    let journal = coord_store
        .get_task_journal(&params.task_id)
        .await
        .unwrap_or(None);

    JsonRpcResponse::success(
        request.id,
        json!({
            "task": task,
            "runs": runs,
            "comments": comments,
            "events": events,
            "artifacts": artifacts,
            "journal": journal,
        }),
    )
}

// =============================================================================
// teams.chat.thread — durable team work thread (tasks + artifacts)
// =============================================================================

/// A single item in the team's durable work thread (task or artifact).
#[derive(Debug, serde::Serialize)]
pub struct ThreadItem {
    /// "task" or "artifact"
    pub kind: String,
    /// Agent that owns the task / submitted the artifact (empty string if unknown).
    pub agent_id: String,
    /// Human-readable title: task subject or artifact title.
    pub title: String,
    /// Task result (falling back to description) or artifact content body.
    pub content: String,
    /// Creation timestamp in milliseconds since epoch, used for chronological merge.
    pub timestamp: i64,
    /// Set only for kind == "artifact".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

/// teams.chat.thread — hydrate a team's DURABLE work thread for the Panel:
/// coordination tasks (subject + owner + result) merged chronologically with
/// their submitted artifacts (deliverables). Read-only, no side effects.
///
/// NOTE: live conversational bubbles (leader/member narrative) are streamed
/// over `team.<id>.*` during the run and are not persisted as queryable team
/// messages, so they are intentionally NOT part of this hydrate — on reload
/// the Panel shows the durable record (tasks + deliverables only). The
/// autonomous dispatcher's task `result` text carries each member's outcome.
pub async fn handle_chat_thread(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>,
    artifact_store: Option<Arc<dyn crate::teams::artifacts::ArtifactStore>>,
) -> JsonRpcResponse {
    debug!("Handling teams.chat.thread request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &store, &params.team_id).await {
        return resp;
    }

    let mut items: Vec<ThreadItem> = Vec::new();

    let tasks = coord_store
        .list_tasks(crate::agents::swarm::tasks::CoordTaskFilter {
            team_id: Some(params.team_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_or_default();

    for t in &tasks {
        // created_at is stored as seconds since epoch; multiply to millis for
        // a common chronological sort basis with artifact DateTime::timestamp_millis().
        let ts = (t.created_at as i64).saturating_mul(1000);

        items.push(ThreadItem {
            kind: "task".to_string(),
            agent_id: t.owner.as_deref().unwrap_or_default().to_string(),
            title: t.subject.clone(),
            content: t.result.clone().unwrap_or_else(|| t.description.clone()),
            timestamp: ts,
            artifact_id: None,
        });

        // One artifact query per task (N+1): a team's task count is small/bounded,
        // and this is a cold hydrate path, not a hot loop.
        // Artifacts submitted for this task — only when the optional store exists.
        // Failure is silently ignored (best-effort observability, mirrors task.trace).
        if let Some(store) = artifact_store.as_ref() {
            if let Ok(arts) = store.get_artifacts_for_task(&t.id).await {
                for a in arts {
                    items.push(ThreadItem {
                        kind: "artifact".to_string(),
                        agent_id: a.agent_id,
                        title: a.title,
                        content: a.content,
                        timestamp: a.created_at.timestamp_millis(),
                        artifact_id: Some(a.id),
                    });
                }
            }
        }
    }

    items.sort_by_key(|i| i.timestamp);
    JsonRpcResponse::success(request.id, serde_json::json!({ "items": items }))
}

// =============================================================================
// teams.chat.history — durable team message transcript as attribution bubbles
// =============================================================================

/// A single bubble item from the team's durable message transcript.
#[derive(serde::Serialize)]
pub(crate) struct ChatHistoryItem {
    pub(crate) from_agent: String,
    pub(crate) content: String,
    pub(crate) msg_type: String,
    /// How the Panel must render this row: `"user"` (right-aligned own bubble),
    /// `"agent"` (attributed left bubble), `"system"` (centered notice chip).
    /// Derived server-side so the Panel never has to re-derive the same
    /// classification from `from_agent`/`msg_type` — one source, one answer.
    pub(crate) kind: &'static str,
    /// Milliseconds since Unix epoch — Panel uses this for chronological order.
    pub(crate) created_at: i64,
    /// The human speaker's raw id, for a `kind == "user"` row whose
    /// `TeamMessage::author_user_id` is set (P2's `send`-time author stamp,
    /// spec §6.2 humanization). `None` for agent/system rows, and for a
    /// human row persisted before this field existed. Mirrors the live
    /// `team.<id>.message` event's `author_user_id` so a replayed row and
    /// its live twin carry identical attribution.
    pub(crate) author_user_id: Option<String>,
    /// Resolved display name for `author_user_id`, via the single derivation
    /// in `speaker.rs` — `None` exactly when `author_user_id` is `None`.
    pub(crate) author_display_name: Option<String>,
}

/// Whether a stored team message belongs in the group-chat transcript at all.
///
/// `team_messages` is a shared bus: besides the conversation itself it carries
/// **directed inbox traffic** — the `TeamNotifier`'s leader digests, the
/// router's thread-escalation hints, discovery pings. Those have explicit
/// `recipients` and were never shown live, so replaying them turned a re-opened
/// group chat into a different (noisier) conversation than the one the user had
/// just been watching. Conversation rows (`MessageType::Message`) and the
/// broadcaster's in-chat notices are recipient-less, which is exactly the
/// discriminator.
fn is_chat_visible(m: &crate::teams::messages::TeamMessage) -> bool {
    use crate::teams::messages::MessageType;
    m.msg_type == MessageType::Message || m.recipients.is_empty()
}

/// Render class for a chat-visible row. `system` covers the broadcaster's
/// in-chat notices ([`crate::teams::broadcast::SYSTEM_HANDLE`]) plus any
/// recipient-less notification that slipped into the transcript.
fn history_kind(m: &crate::teams::messages::TeamMessage) -> &'static str {
    use crate::teams::messages::MessageType;
    if m.from_agent == crate::teams::broadcast::RESERVED_USER_HANDLE {
        "user"
    } else if m.from_agent == crate::teams::broadcast::SYSTEM_HANDLE
        || m.msg_type != MessageType::Message
    {
        "system"
    } else {
        "agent"
    }
}

/// Map a flat list of [`TeamMessage`] values to [`ChatHistoryItem`] values,
/// sorted chronologically, dropping directed inbox traffic. Extracted as a free
/// function so it is unit-testable without any async store.
///
/// `labels` is the caller's already-resolved `author_user_id -> display_name`
/// map (built once via [`speaker::resolve_labels_for_messages`] over the same
/// batch, per Ruling P2 — this function does not open a `SecurityStore`
/// lookup of its own).
pub(crate) fn map_history(
    msgs: Vec<crate::teams::messages::TeamMessage>,
    labels: &std::collections::HashMap<String, String>,
) -> Vec<ChatHistoryItem> {
    let mut items: Vec<ChatHistoryItem> = msgs
        .into_iter()
        .filter(is_chat_visible)
        .map(|m| {
            let kind = history_kind(&m);
            // `speaker_label`'s `Some(_)` arm never consults `from_agent`, so
            // the row's real `from_agent` is fine to pass unconditionally.
            let author_display_name = m.author_user_id.as_deref().map(|uid| {
                crate::teams::broadcast::speaker::speaker_label(&m.from_agent, Some(uid), labels)
            });
            ChatHistoryItem {
                kind,
                from_agent: m.from_agent,
                author_user_id: m.author_user_id,
                author_display_name,
                content: m.content,
                msg_type: m.msg_type.as_str().to_string(),
                created_at: m.created_at.timestamp_millis(),
            }
        })
        .collect();
    items.sort_by_key(|i| i.created_at);
    items
}

/// teams.chat.history — replay the durable group-chat transcript as attribution
/// bubbles. Returns `{ "items": [...] }` chronologically. Unknown or empty teams
/// return an empty items array rather than an error (idempotent cold-open).
pub async fn handle_chat_history(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    msg_store: Arc<dyn crate::teams::messages::MessageStore>,
    security_store: Arc<crate::gateway::security::SecurityStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.chat.history request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &store, &params.team_id).await {
        return resp;
    }

    // 200 chat bubbles is ample for the Panel's initial hydrate. The store
    // query is a raw tail over `team_messages`, which also carries directed
    // inbox traffic that `map_history` drops — so over-fetch and trim AFTER
    // filtering, or a team whose leader got a burst of notifier digests would
    // hydrate with a near-empty conversation.
    const CHAT_BUBBLE_LIMIT: usize = 200;
    const RAW_FETCH_LIMIT: usize = CHAT_BUBBLE_LIMIT * 3;
    let msgs = msg_store
        .list_team_messages(&params.team_id, RAW_FETCH_LIMIT)
        .await
        .unwrap_or_default();

    // Ruling P2: resolve every distinct human author's display name through
    // the single derivation in `speaker.rs` (batched once here, not per row
    // and not via a fresh `SecurityStore::get_user` loop) — same injection
    // point `teams.chat.send` uses for the live event.
    let labels =
        crate::teams::broadcast::speaker::resolve_labels_for_messages(Some(&security_store), &msgs);
    let mut items = map_history(msgs, &labels);
    // Keep the newest window; `map_history` returns oldest-first.
    if items.len() > CHAT_BUBBLE_LIMIT {
        items.drain(..items.len() - CHAT_BUBBLE_LIMIT);
    }
    JsonRpcResponse::success(request.id, serde_json::json!({ "items": items }))
}

// =============================================================================
// teams.task.journal.{get,list} — R3 T3 read surface for exit journals
// =============================================================================

/// teams.task.journal.get — fetch the journal for one task, or null.
pub async fn handle_task_journal_get(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.journal.get request");
    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_task(request.id.clone(), &store, &coord_store, &params.task_id).await {
        return resp;
    }
    match coord_store.get_task_journal(&params.task_id).await {
        Ok(journal) => JsonRpcResponse::success(request.id, json!({ "journal": journal })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to load journal for task '{}': {e}", params.task_id),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct TeamJournalListParams {
    pub team_id: String,
}

/// teams.task.journal.list — list all journals for a team, newest first.
pub async fn handle_task_journal_list(
    request: JsonRpcRequest,
    store: Arc<dyn crate::teams::TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.journal.list request");
    let params: TeamJournalListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_team(request.id.clone(), &store, &params.team_id).await {
        return resp;
    }
    match coord_store.list_team_journals(&params.team_id).await {
        Ok(journals) => JsonRpcResponse::success(request.id, json!({ "journals": journals })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list journals for team '{}': {e}", params.team_id),
        ),
    }
}
