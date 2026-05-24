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

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStore};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;

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
