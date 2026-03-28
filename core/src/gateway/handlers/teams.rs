//! Team Management Handlers
//!
//! RPC handlers for team CRUD and membership operations:
//! - teams.list: List all teams with summaries
//! - teams.get: Get full team detail (team + members + tasks)
//! - teams.disband: Mark a team as disbanded
//! - teams.delete: Permanently delete a disbanded team
//! - agents.teams: List all teams an agent belongs to

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::sync_primitives::Arc;
use crate::teams::TeamStore;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, RESOURCE_NOT_FOUND};
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
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
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

    let tasks = match store.get_tasks(&params.team_id).await {
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
pub async fn handle_disband(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
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
pub async fn handle_delete(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
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
