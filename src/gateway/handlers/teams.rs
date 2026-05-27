//! Team Management Handlers
//!
//! RPC handlers for team CRUD and membership operations:
//! - teams.list: List all teams with summaries
//! - teams.get: Get full team detail (team + members + tasks)
//! - teams.disband: Mark a team as disbanded
//! - teams.delete: Permanently delete a disbanded team
//! - agents.teams: List all teams an agent belongs to
//! - teams.list_tasks / teams.create_task / teams.update_task: kanban-facing
//!   CoordTask operations
//! - teams.snapshot.{create,list,get,restore,delete}: snapshot lifecycle —
//!   thin direct surface in addition to the `team_snapshot` builtin tool

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict,
    ReviewerKind,
};
use crate::resilience::{AgentUsageTotal, StateDatabase};
use crate::sync_primitives::Arc;
use crate::teams::snapshots::{capture_snapshot, restore_snapshot, SqliteSnapshotStore};
use crate::teams::{NewTeamMember, TeamMemberKind, TeamStore};

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use super::parse_params;

// =============================================================================
// Request Parameters
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TeamIdParams {
    pub team_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentIdParams {
    pub agent_id: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle teams.list — list all teams as lightweight summaries
pub async fn handle_list(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    debug!("Handling teams.list request");

    match store.list_teams().await {
        Ok(teams) => JsonRpcResponse::success(request.id, json!({ "teams": teams })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list teams: {}", e),
        ),
    }
}

/// Handle teams.get — get full team detail: team record, members, and tasks
pub async fn handle_get(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.get request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let team = match store.get_team(&params.team_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Team '{}' not found", params.team_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get team '{}': {}", params.team_id, e),
            )
        }
    };

    let members = match store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get members for team '{}': {}", params.team_id, e),
            )
        }
    };

    let tasks = match coord_store
        .list_tasks(CoordTaskFilter {
            team_id: Some(params.team_id.clone()),
            ..Default::default()
        })
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get tasks for team '{}': {}", params.team_id, e),
            )
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "team": team, "members": members, "tasks": tasks }),
    )
}

/// Handle teams.disband — mark a team as disbanded
pub async fn handle_disband(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    debug!("Handling teams.disband request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.disband_team(&params.team_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to disband team '{}': {}", params.team_id, e),
        ),
    }
}

/// Handle teams.delete — permanently delete a disbanded team
pub async fn handle_delete(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    debug!("Handling teams.delete request");

    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.delete_team(&params.team_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete team '{}': {}", params.team_id, e),
        ),
    }
}

/// Handle agents.teams — list all teams an agent belongs to (as leader or member)
pub async fn handle_agent_teams(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling agents.teams request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.get_agent_teams(&params.agent_id).await {
        Ok(teams) => JsonRpcResponse::success(request.id, json!({ "teams": teams })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get teams for agent '{}': {}", params.agent_id, e),
        ),
    }
}

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

/// Handle teams.list_tasks — list all CoordTasks for a team with optional status/owner filter
pub async fn handle_list_tasks(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_tasks request");

    let params: ListTasksParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    use crate::agents::swarm::tasks::CoordTaskStatus;
    let status_filter = params.status.as_deref().and_then(|s| match s {
        "pending" => Some(CoordTaskStatus::Pending),
        "blocked" => Some(CoordTaskStatus::Blocked),
        "in_progress" => Some(CoordTaskStatus::InProgress),
        "completed" => Some(CoordTaskStatus::Completed),
        "failed" => Some(CoordTaskStatus::Failed),
        "cancelled" => Some(CoordTaskStatus::Cancelled),
        _ => None,
    });

    let filter = CoordTaskFilter {
        team_id: Some(params.team_id.clone()),
        status: status_filter,
        owner: params.owner.clone(),
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

/// Handle teams.update_task — patch a CoordTask. Topic emission happens
/// inside the store, so any subscriber sees the change automatically.
pub async fn handle_update_task(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.update_task request");

    let params: UpdateTaskParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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

    let update = CoordTaskUpdate {
        status,
        owner: params.owner.clone(),
        result: params.result.clone(),
        metadata: params.metadata.clone(),
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

/// Handle teams.create_task — create a CoordTask on a team's board. Topic
/// emission happens inside the store, so the kanban panel refreshes itself.
pub async fn handle_create_task(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.create_task request");

    let params: CreateTaskParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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

/// Handle teams.list_task_runs — return all run records for a task, oldest
/// first. The drawer renders this as the "Runs" timeline.
pub async fn handle_list_task_runs(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_runs request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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

/// Handle teams.add_task_comment — append a free-text note to a task. Used
/// by workers to leave handoff context and by panel users to annotate state.
pub async fn handle_add_task_comment(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.add_task_comment request");

    let params: AddTaskCommentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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
            format!(
                "Failed to add comment to task '{}': {}",
                params.task_id, e
            ),
        ),
    }
}

/// Handle teams.list_task_comments — return all comments for a task,
/// oldest first.
pub async fn handle_list_task_comments(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_comments request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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

/// Handle teams.list_task_events — return team events whose payload references
/// `task_id`. Walks the team's event log and filters in-memory; cheap for the
/// kanban-scale event volumes in practice (Aleph's logger isn't a firehose).
///
/// Order: oldest first, matching the drawer's "timeline reads top-to-bottom"
/// convention.
pub async fn handle_list_task_events(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
    event_store: Arc<dyn crate::teams::events::EventLogStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.list_task_events request");

    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

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
                format!("Failed to load events for team '{}': {}", team_id, e),
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
                .map(|tid| tid == params.task_id)
                .unwrap_or(false)
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "events": filtered }))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;

    async fn coord_store() -> Arc<dyn CoordTaskStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        Arc::new(store)
    }

    fn create_req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "teams.create_task".to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn create_task_trims_subject_and_returns_task() {
        let store = coord_store().await;
        let resp = handle_create_task(
            create_req(json!({"team_id": "T", "subject": "  Ship it  ", "priority": "high"})),
            store,
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result present");
        let task = &result["task"];
        assert_eq!(task["subject"], "Ship it");
        assert_eq!(task["team_id"], "T");
        assert_eq!(task["priority"], "high");
        assert_eq!(task["status"], "pending");
    }

    #[tokio::test]
    async fn create_task_rejects_blank_subject() {
        let store = coord_store().await;
        let resp =
            handle_create_task(create_req(json!({"team_id": "T", "subject": "   "})), store).await;

        let err = resp.error.expect("expected an error");
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn create_task_rejects_unknown_priority() {
        let store = coord_store().await;
        let resp = handle_create_task(
            create_req(json!({"team_id": "T", "subject": "x", "priority": "urgent"})),
            store,
        )
        .await;

        let err = resp.error.expect("expected an error");
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn create_task_with_owner_auto_injects_managed_by_dispatcher() {
        let store = coord_store().await;
        let resp = handle_create_task(
            create_req(json!({
                "team_id": "T",
                "subject": "auto-dispatch",
                "owner": "worker-a",
            })),
            store,
        )
        .await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let task = resp.result.expect("result")["task"].clone();
        assert_eq!(
            task["metadata"]["managed_by"], "dispatcher",
            "owner-bearing tasks must be flagged for the dispatcher loop or they silently never run; metadata={:?}",
            task["metadata"]
        );
    }

    #[tokio::test]
    async fn create_task_without_owner_skips_managed_by_marker() {
        // Orphan tasks (no owner yet) shouldn't auto-claim the dispatcher
        // namespace — they're parked until a leader assigns them.
        let store = coord_store().await;
        let resp = handle_create_task(
            create_req(json!({ "team_id": "T", "subject": "orphan" })),
            store,
        )
        .await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let task = resp.result.expect("result")["task"].clone();
        assert!(
            task["metadata"].get("managed_by").is_none(),
            "tasks without an owner must NOT carry the dispatcher marker; got {:?}",
            task["metadata"]
        );
    }

    #[tokio::test]
    async fn create_task_respects_caller_supplied_managed_by_override() {
        let store = coord_store().await;
        let resp = handle_create_task(
            create_req(json!({
                "team_id": "T",
                "subject": "manual",
                "owner": "worker-x",
                "metadata": {"managed_by": "team_delegate"},
            })),
            store,
        )
        .await;
        assert!(resp.error.is_none());
        let task = resp.result.expect("result")["task"].clone();
        assert_eq!(
            task["metadata"]["managed_by"], "team_delegate",
            "caller-supplied managed_by must win over auto-injection"
        );
    }
}

// =============================================================================
// teams.list_templates — discover available team templates (built-in + user)
// =============================================================================

/// Handle `teams.list_templates` — no params; returns the discoverable
/// templates' metadata so a UI can render a picker. Materialization itself
/// happens via the `team_from_template` builtin tool (R8 — tool-for-everything)
/// so we don't double-plumb the heavy dep set through the gateway.
pub async fn handle_list_templates(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling teams.list_templates request");

    let registry = crate::teams::templates::TemplateRegistry::discover(
        &crate::teams::templates::loader::default_user_dir(),
    );

    let entries: Vec<serde_json::Value> = registry
        .list()
        .map(|tpl| {
            json!({
                "name": tpl.name,
                "description": tpl.description,
                "default_goal": tpl.default_goal,
                "leader_id": tpl.leader.id,
                "leader_role": tpl.leader.role,
                "member_count": tpl.members.len(),
                "task_count": tpl.tasks.len(),
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "templates": entries }))
}

// =============================================================================
// teams.snapshot.* — snapshot lifecycle as direct RPC
//
// Mirrors the `team_snapshot` builtin tool so panels and external callers can
// hit the snapshot store without going through tool-invoke. Same backing
// functions (capture_snapshot / restore_snapshot + SnapshotStore methods) so
// behaviour, dry-run defaults, and edge-restoration semantics are identical.
// =============================================================================

// =============================================================================
// ACP Member Operations (teams.acp_member.{add,remove,list})
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AcpMemberAddParams {
    pub team_id: String,
    pub harness_id: String,
    pub cwd: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default = "default_acp_role")]
    pub role: String,
}

fn default_acp_role() -> String {
    "acp-worker".to_string()
}

#[derive(Debug, Deserialize)]
pub struct AcpMemberRemoveParams {
    pub team_id: String,
    /// The synthetic id returned by `teams.acp_member.add` — also the value
    /// stored as `coord_tasks.owner` for tasks assigned to this member.
    pub agent_id: String,
}

/// teams.acp_member.add — register an external coding CLI session as a
/// team member. Subsequent tasks created with `owner = <agent_id>` will be
/// dispatched through the ACP adapter pool instead of the in-process agent
/// registry. Idempotent: re-adding the same (team, harness, cwd, name)
/// returns the existing row.
pub async fn handle_acp_member_add(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.add request");
    let params: AcpMemberAddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let new_member = NewTeamMember::for_acp_session(
        params.team_id.clone(),
        params.harness_id,
        params.cwd,
        params.session_name,
        params.role,
    );

    match store.add_member(new_member).await {
        Ok(member) => JsonRpcResponse::success(request.id, json!({ "member": member })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to add ACP member to team '{}': {}", params.team_id, e),
        ),
    }
}

/// teams.acp_member.remove — detach an ACP-backed member from a team. The
/// underlying ACP session in the pool is **not** killed (other teams may
/// still reference it); use `acp.sessions.shutdown` for that.
pub async fn handle_acp_member_remove(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.remove request");
    let params: AcpMemberRemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Guard: refuse to remove a non-ACP row via this RPC so the caller
    // doesn't accidentally drop an in-process agent.
    let members = match store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read team '{}' members: {}", params.team_id, e),
            )
        }
    };
    match members.iter().find(|m| m.agent_id == params.agent_id) {
        Some(m) if m.kind == TeamMemberKind::AcpSession => {}
        Some(_) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Member '{}' is not ACP-backed; use teams.remove_member",
                    params.agent_id
                ),
            )
        }
        None => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!(
                    "Member '{}' not found in team '{}'",
                    params.agent_id, params.team_id
                ),
            )
        }
    }

    match store
        .remove_member(&params.team_id, &params.agent_id)
        .await
    {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove ACP member: {e}"),
        ),
    }
}

/// teams.acp_member.list — return only the ACP-backed members of a team.
/// Convenience filter — `teams.get` returns mixed-kind members.
pub async fn handle_acp_member_list(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.list request");
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.get_members(&params.team_id).await {
        Ok(members) => {
            let acp_only: Vec<_> = members
                .into_iter()
                .filter(|m| m.kind == TeamMemberKind::AcpSession)
                .collect();
            JsonRpcResponse::success(request.id, json!({ "members": acp_only }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list ACP members for team '{}': {}", params.team_id, e),
        ),
    }
}

// =============================================================================
// Workflow Step Review (Phase C — openteams-parity)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct WorkflowStepReviewParams {
    pub task_id: String,
    /// Reviewer category. `user` is set by the panel; the lead agent
    /// uses `lead_agent`; system auto-approval uses `auto`.
    #[serde(default = "default_reviewer_kind")]
    pub reviewer_kind: String,
    #[serde(default)]
    pub reviewer_id: Option<String>,
    /// Optional free-text comment appended as a task comment.
    #[serde(default)]
    pub comment: Option<String>,
}

fn default_reviewer_kind() -> String {
    "user".to_string()
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRetryStepParams {
    pub task_id: String,
}

fn parse_reviewer_kind(s: &str) -> Result<ReviewerKind, &'static str> {
    ReviewerKind::from_stored(s).ok_or("reviewer_kind must be one of: user, lead_agent, auto")
}

/// teams.workflow.approve_step — stamp the latest run as approved and
/// transition the task to Completed so downstream dependents unblock.
/// Idempotent on Completed tasks (re-approve is a no-op). Refuses to
/// approve tasks that have not yet finished a run.
pub async fn handle_workflow_approve_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.approve_step request");
    let params: WorkflowStepReviewParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let reviewer_kind = match parse_reviewer_kind(&params.reviewer_kind) {
        Ok(k) => k,
        Err(msg) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg.to_string())
        }
    };

    if let Err(e) = coord_store
        .record_run_review(
            &params.task_id,
            ReviewVerdict::Approved,
            reviewer_kind,
            params.reviewer_id.as_deref(),
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to record review: {e}"),
        );
    }

    // Transition status: WaitingReview / InProgress → Completed.
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to mark task completed: {e}"),
        );
    }

    if let Some(comment) = params.comment.as_deref().filter(|c| !c.trim().is_empty()) {
        let author = params
            .reviewer_id
            .clone()
            .unwrap_or_else(|| format!("review:{}", reviewer_kind.as_str()));
        if let Err(e) = coord_store
            .add_task_comment(&params.task_id, &author, comment)
            .await
        {
            tracing::warn!(error = %e, "approve_step: failed to record review comment");
        }
    }

    JsonRpcResponse::success(request.id, json!({ "status": "completed" }))
}

/// teams.workflow.reject_step — stamp the latest run as rejected and
/// transition the task to Failed. Downstream dependents stay blocked.
pub async fn handle_workflow_reject_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.reject_step request");
    let params: WorkflowStepReviewParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let reviewer_kind = match parse_reviewer_kind(&params.reviewer_kind) {
        Ok(k) => k,
        Err(msg) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg.to_string())
        }
    };

    if let Err(e) = coord_store
        .record_run_review(
            &params.task_id,
            ReviewVerdict::Rejected,
            reviewer_kind,
            params.reviewer_id.as_deref(),
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to record review: {e}"),
        );
    }
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                result: params.comment.clone(),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to mark task failed: {e}"),
        );
    }
    if let Some(comment) = params.comment.as_deref().filter(|c| !c.trim().is_empty()) {
        let author = params
            .reviewer_id
            .clone()
            .unwrap_or_else(|| format!("review:{}", reviewer_kind.as_str()));
        let _ = coord_store
            .add_task_comment(&params.task_id, &author, comment)
            .await;
    }
    JsonRpcResponse::success(request.id, json!({ "status": "failed" }))
}

/// teams.workflow.retry_step — re-queue a failed / rejected step. Clears
/// the lock + result fields and resets status to Pending so the
/// dispatcher (or `team_delegate`) can re-run. Prior runs stay in history.
pub async fn handle_workflow_retry_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.retry_step request");
    let params: WorkflowRetryStepParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Pending),
                result: Some(String::new()),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reset task to pending: {e}"),
        );
    }
    // Best-effort lock release — failure here is non-fatal (lock may
    // have expired naturally or never been acquired).
    let _ = coord_store.release_lock(&params.task_id, "").await;
    JsonRpcResponse::success(request.id, json!({ "status": "pending" }))
}

#[derive(Debug, Deserialize)]
pub struct SnapshotCreateParams {
    pub team_id: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SnapshotListParams {
    #[serde(default)]
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotIdParams {
    pub snapshot_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotRestoreParams {
    pub snapshot_id: String,
    /// `false` (default) → dry-run, returns the diff without mutation.
    /// `true` → applies the snapshot, including dependency-edge restoration.
    #[serde(default)]
    pub apply: bool,
}

/// teams.snapshot.create — capture a new snapshot of `team_id`'s current state.
pub async fn handle_snapshot_create(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.create request");
    let params: SnapshotCreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let tag = params.tag.unwrap_or_default();
    let note = params.note.unwrap_or_default();
    match capture_snapshot(
        snapshot_store.as_ref(),
        team_store.as_ref(),
        coord_store.as_ref(),
        &params.team_id,
        &tag,
        &note,
    )
    .await
    {
        Ok(out) => JsonRpcResponse::success(request.id, json!(out)),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot create failed: {e}"),
        ),
    }
}

/// teams.snapshot.list — newest-first metadata listing, optionally filtered.
pub async fn handle_snapshot_list(
    request: JsonRpcRequest,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.list request");
    // List is the only snapshot RPC that allows missing params (no filter).
    let params: SnapshotListParams = match request.params.as_ref() {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("invalid params: {e}"),
                )
            }
        },
        None => SnapshotListParams::default(),
    };
    match snapshot_store.list(params.team_id.as_deref()).await {
        Ok(metas) => JsonRpcResponse::success(request.id, json!({ "snapshots": metas })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot list failed: {e}"),
        ),
    }
}

/// teams.snapshot.get — load the full payload by id.
pub async fn handle_snapshot_get(
    request: JsonRpcRequest,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.get request");
    let params: SnapshotIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match snapshot_store.get(&params.snapshot_id).await {
        Ok(Some((meta, payload))) => {
            JsonRpcResponse::success(request.id, json!({ "meta": meta, "payload": payload }))
        }
        Ok(None) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("snapshot '{}' not found", params.snapshot_id),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot get failed: {e}"),
        ),
    }
}

/// teams.snapshot.restore — apply or dry-run; returns the diff either way.
pub async fn handle_snapshot_restore(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.restore request");
    let params: SnapshotRestoreParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match restore_snapshot(
        snapshot_store.as_ref(),
        team_store.as_ref(),
        coord_store.as_ref(),
        &params.snapshot_id,
        !params.apply,
    )
    .await
    {
        Ok(diff) => JsonRpcResponse::success(request.id, json!(diff)),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot restore failed: {e}"),
        ),
    }
}

// =============================================================================
// teams.usage — per-team provider token aggregation (F5)
// =============================================================================

/// Params for `teams.usage`. `since` / `until` are optional epoch-second
/// bounds. They default to "all time" when absent.
#[derive(Debug, Deserialize)]
pub struct UsageParams {
    pub team_id: String,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub until: Option<i64>,
}

/// teams.usage — returns provider-token usage aggregated across all members
/// of `team_id`, plus a per-agent breakdown. Pure read-side: derived from
/// `task_traces` ProviderUsage events that are already persisted by the
/// gateway execution engine.
///
/// Cost is intentionally not computed here — tokens are factual, cost is
/// rate-card policy that lives in the provider config / UI layer.
pub async fn handle_usage(
    request: JsonRpcRequest,
    team_store: Arc<dyn TeamStore>,
    state_db: Arc<StateDatabase>,
) -> JsonRpcResponse {
    debug!("Handling teams.usage request");
    let params: UsageParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Step 1 — resolve team members. Both "team not found" and "team exists
    // but is empty" are distinguishable here so the response can disambiguate.
    let team_exists = match team_store.get_team(&params.team_id).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Team '{}' not found", params.team_id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load team '{}': {}", params.team_id, e),
            );
        }
    };
    let _ = team_exists; // pure existence check; member list drives the join

    let members = match team_store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load members for '{}': {}", params.team_id, e),
            );
        }
    };
    let agent_ids: Vec<String> = members.iter().map(|m| m.agent_id.clone()).collect();

    // Step 2 — aggregate usage across the team's members.
    let per_agent: Vec<AgentUsageTotal> = match state_db
        .aggregate_usage_by_agents(&agent_ids, params.since, params.until)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Usage aggregation failed for '{}': {}", params.team_id, e),
            );
        }
    };

    // Step 3 — fold into team totals. Saturating sums match the harness
    // `TokenBreakdown::total()` semantics.
    let mut total_calls: u64 = 0;
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_creation: u64 = 0;
    let mut total_reasoning: u64 = 0;
    for row in &per_agent {
        total_calls = total_calls.saturating_add(row.call_count);
        total_input = total_input.saturating_add(row.input_tokens);
        total_output = total_output.saturating_add(row.output_tokens);
        total_cache_read = total_cache_read.saturating_add(row.cache_read_tokens);
        total_cache_creation = total_cache_creation.saturating_add(row.cache_creation_tokens);
        total_reasoning = total_reasoning.saturating_add(row.reasoning_tokens);
    }
    // Reuse the per-row method so the rollup denominator matches what each
    // AgentUsageTotal would report on its own.
    let cache_hit_ratio = AgentUsageTotal {
        agent_id: String::new(),
        call_count: total_calls,
        input_tokens: total_input,
        output_tokens: total_output,
        cache_read_tokens: total_cache_read,
        cache_creation_tokens: total_cache_creation,
        reasoning_tokens: total_reasoning,
    }
    .cache_hit_ratio();

    JsonRpcResponse::success(
        request.id,
        json!({
            "team_id": params.team_id,
            "since": params.since,
            "until": params.until,
            "member_count": agent_ids.len(),
            "total": {
                "call_count": total_calls,
                "input_tokens": total_input,
                "output_tokens": total_output,
                "cache_read_tokens": total_cache_read,
                "cache_creation_tokens": total_cache_creation,
                "reasoning_tokens": total_reasoning,
                "cache_hit_ratio": cache_hit_ratio,
            },
            "per_agent": per_agent,
        }),
    )
}

/// teams.snapshot.delete — idempotent removal. Returns `existed: bool`.
pub async fn handle_snapshot_delete(
    request: JsonRpcRequest,
    snapshot_store: Arc<SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.snapshot.delete request");
    let params: SnapshotIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match snapshot_store.delete(&params.snapshot_id).await {
        Ok(existed) => JsonRpcResponse::success(
            request.id,
            json!({ "snapshot_id": params.snapshot_id, "existed": existed }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("snapshot delete failed: {e}"),
        ),
    }
}

#[cfg(test)]
mod snapshot_handler_tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
    use crate::teams::{NewTeam, NewTeamMember, SqliteTeamStore};
    use rusqlite::Connection;

    async fn setup() -> (
        Arc<dyn TeamStore>,
        Arc<dyn CoordTaskStore>,
        Arc<SqliteSnapshotStore>,
        String,
    ) {
        let coord_conn = Connection::open_in_memory().unwrap();
        let coord = Arc::new(SqliteCoordTaskStore::new(coord_conn));
        coord.migrate().await.unwrap();
        let snap = Arc::new(SqliteSnapshotStore::new_from_shared(coord.connection_handle()));

        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "Alpha".into(),
                description: "rpc test".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        teams
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "leader".into(),
                role: "leader".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let teams_arc: Arc<dyn TeamStore> = teams;
        let coord_arc: Arc<dyn CoordTaskStore> = coord;
        (teams_arc, coord_arc, snap, team.id)
    }

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: Some(serde_json::Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn create_list_get_restore_delete_full_lifecycle() {
        let (teams, coord, snap, team_id) = setup().await;

        // create
        let resp = handle_snapshot_create(
            req(
                "teams.snapshot.create",
                json!({ "team_id": team_id, "tag": "v1", "note": "first" }),
            ),
            teams.clone(),
            coord.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "create error: {:?}", resp.error);
        let sid = resp.result.as_ref().unwrap()["snapshot_id"]
            .as_str()
            .unwrap()
            .to_string();

        // list
        let resp = handle_snapshot_list(
            req("teams.snapshot.list", json!({ "team_id": team_id })),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        let arr = resp.result.as_ref().unwrap()["snapshots"]
            .as_array()
            .unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"].as_str().unwrap(), sid);

        // get
        let resp =
            handle_snapshot_get(req("teams.snapshot.get", json!({ "snapshot_id": sid })), snap.clone())
                .await;
        assert!(resp.error.is_none());
        let payload = resp.result.as_ref().unwrap()["payload"].clone();
        assert_eq!(payload["team"]["id"].as_str().unwrap(), team_id);

        // restore — dry-run (apply omitted ⇒ default false)
        let resp = handle_snapshot_restore(
            req("teams.snapshot.restore", json!({ "snapshot_id": sid })),
            teams.clone(),
            coord.clone(),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(
            resp.result.as_ref().unwrap()["dry_run"].as_bool().unwrap(),
            true,
            "default apply must be false → dry_run true"
        );

        // delete
        let resp = handle_snapshot_delete(
            req("teams.snapshot.delete", json!({ "snapshot_id": sid })),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(
            resp.result.as_ref().unwrap()["existed"].as_bool().unwrap(),
            true
        );

        // delete again → existed:false (idempotent)
        let resp = handle_snapshot_delete(
            req("teams.snapshot.delete", json!({ "snapshot_id": sid })),
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(
            resp.result.as_ref().unwrap()["existed"].as_bool().unwrap(),
            false
        );

        // get after delete → not found
        let resp =
            handle_snapshot_get(req("teams.snapshot.get", json!({ "snapshot_id": sid })), snap.clone())
                .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn list_works_without_params() {
        let (_teams, _coord, snap, _team_id) = setup().await;
        let resp = handle_snapshot_list(
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "teams.snapshot.list".into(),
                params: None,
                id: Some(serde_json::Value::Number(1.into())),
            },
            snap.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        assert!(
            resp.result.as_ref().unwrap()["snapshots"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(test)]
mod usage_handler_tests {
    use super::*;
    use crate::resilience::{AgentTask, RiskLevel, StateDatabase, TaskTrace};
    use crate::teams::{NewTeam, NewTeamMember, SqliteTeamStore};
    use aleph_protocol::AgentTraceEvent;
    use rusqlite::Connection;

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
            id: Some(serde_json::Value::Number(1.into())),
        }
    }

    async fn setup_team_with_usage() -> (Arc<dyn TeamStore>, Arc<StateDatabase>, String) {
        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "UsageTeam".into(),
                description: "usage rpc".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        for who in ["leader", "worker"] {
            teams
                .add_member(NewTeamMember {
                    team_id: team.id.clone(),
                    agent_id: who.into(),
                    role: who.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
        }

        let db = Arc::new(StateDatabase::in_memory().unwrap());
        // Seed task + provider_usage rows for leader & worker, plus one row
        // for an unrelated agent that must be filtered out.
        for (task, agent, input, ts) in [
            ("t1", "leader", 100u32, 1000i64),
            ("t1", "leader", 200, 1100),
            ("t1", "worker", 50, 1200),
            ("t2", "outsider", 999, 1300),
        ] {
            let _ = db
                .insert_agent_task(&AgentTask::new(task, "s", "coder", "x", RiskLevel::Low))
                .await;
            db.insert_trace(&TaskTrace {
                id: 0,
                task_id: task.into(),
                step_index: ts as u32,
                event: AgentTraceEvent::ProviderUsage {
                    agent_id: agent.into(),
                    input_tokens: input,
                    output_tokens: input / 2,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    thinking_tokens: None,
                },
                timestamp: ts,
            })
            .await
            .unwrap();
        }

        let teams_arc: Arc<dyn TeamStore> = teams;
        (teams_arc, db, team.id)
    }

    #[tokio::test]
    async fn usage_aggregates_members_and_excludes_outsiders() {
        let (teams, db, team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": team_id })),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "usage error: {:?}", resp.error);
        let result = resp.result.as_ref().unwrap();
        assert_eq!(result["member_count"].as_u64().unwrap(), 2);
        // 100 + 200 (leader) + 50 (worker) = 350; outsider's 999 must be ignored.
        assert_eq!(result["total"]["input_tokens"].as_u64().unwrap(), 350);
        // outputs = input/2 each → 50 + 100 + 25 = 175
        assert_eq!(result["total"]["output_tokens"].as_u64().unwrap(), 175);
        assert_eq!(result["total"]["call_count"].as_u64().unwrap(), 3);
        // Seed rows carry cache_read = None → 0, with non-zero input → ratio 0.0
        // (distinguishable from "no data" None).
        assert_eq!(result["total"]["cache_hit_ratio"].as_f64().unwrap(), 0.0);
        let per_agent = result["per_agent"].as_array().unwrap();
        assert_eq!(per_agent.len(), 2, "outsider must not appear");
    }

    /// Seed rows with real cache_read counts so the team-level cache_hit_ratio
    /// surfaces a non-zero number on the wire. Locks in the per-row vs. rollup
    /// denominator parity claimed by the comment above `handle_usage`.
    #[tokio::test]
    async fn usage_reports_cache_hit_ratio_when_cache_reads_present() {
        let teams_conn = Connection::open_in_memory().unwrap();
        let teams = Arc::new(SqliteTeamStore::new(teams_conn));
        teams.migrate().await.unwrap();
        let team = teams
            .create_team(NewTeam {
                name: "Cached".into(),
                description: "ratio test".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        teams
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "leader".into(),
                role: "leader".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let _ = db
            .insert_agent_task(&AgentTask::new("t1", "s", "coder", "x", RiskLevel::Low))
            .await;
        // OpenAI/DeepSeek shape: input is the total and includes the cached
        // portion. ratio = 80 / 100 = 0.8 per row, also at the rollup.
        db.insert_trace(&TaskTrace {
            id: 0,
            task_id: "t1".into(),
            step_index: 0,
            event: AgentTraceEvent::ProviderUsage {
                agent_id: "leader".into(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: Some(80),
                cache_creation_tokens: None,
                thinking_tokens: None,
            },
            timestamp: 1000,
        })
        .await
        .unwrap();

        let teams_arc: Arc<dyn TeamStore> = teams;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": team.id })),
            teams_arc,
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let ratio = resp.result.as_ref().unwrap()["total"]["cache_hit_ratio"]
            .as_f64()
            .expect("ratio must be a number");
        assert!((ratio - 0.8).abs() < 1e-9, "expected 0.8, got {ratio}");
    }

    #[tokio::test]
    async fn usage_honours_since_until_window() {
        let (teams, db, team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req(
                "teams.usage",
                json!({ "team_id": team_id, "since": 1050, "until": 1150 }),
            ),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_none());
        let total = resp.result.as_ref().unwrap()["total"].clone();
        // Only leader's 1100-stamped row sits inside [1050, 1150].
        assert_eq!(total["input_tokens"].as_u64().unwrap(), 200);
        assert_eq!(total["call_count"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn usage_returns_not_found_for_unknown_team() {
        let (teams, db, _team_id) = setup_team_with_usage().await;
        let resp = handle_usage(
            req("teams.usage", json!({ "team_id": "ghost" })),
            teams.clone(),
            db.clone(),
        )
        .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, RESOURCE_NOT_FOUND);
    }
}

#[cfg(test)]
mod template_handler_tests {
    use super::*;

    #[tokio::test]
    async fn list_templates_returns_builtins() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "teams.list_templates".into(),
            params: None,
            id: Some(serde_json::Value::Number(1.into())),
        };
        let resp = handle_list_templates(req).await;
        assert!(resp.error.is_none(), "expected ok, got {:?}", resp.error);
        let entries = resp.result.expect("result")["templates"].clone();
        let names: Vec<String> = entries
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|v| v["name"].as_str().map(String::from))
            .collect();
        assert!(names.contains(&"software-dev".to_string()));
        assert!(names.contains(&"code-review".to_string()));
        assert!(names.contains(&"research-paper".to_string()));
        assert!(names.contains(&"strategy-room".to_string()));
    }
}
