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

    let new_task = NewCoordTask {
        team_id: Some(params.team_id.clone()),
        subject: subject.to_string(),
        description: params.description.unwrap_or_default(),
        owner: params.owner.filter(|s| !s.trim().is_empty()),
        priority,
        blocked_by: params.blocked_by.unwrap_or_default(),
        metadata: params
            .metadata
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
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
}
