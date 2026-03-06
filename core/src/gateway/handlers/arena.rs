//! Arena RPC handlers — create, query, and settle SharedArena instances.

use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use crate::arena::{
    ArenaId, ArenaManifest, ArenaManager, ArenaPermissions, CoordinationStrategy,
    Participant, ParticipantRole, StageSpec,
};
use crate::sync_primitives::{Arc, RwLock};

use super::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Shared ArenaManager type alias used by handler registration.
pub type SharedArenaManager = Arc<RwLock<ArenaManager>>;

// =============================================================================
// Params
// =============================================================================

#[derive(Deserialize)]
pub struct CreateArenaParams {
    pub goal: String,
    pub strategy: String, // "peer" or "pipeline"
    pub participants: Vec<String>, // agent IDs
    #[serde(default)]
    pub coordinator: Option<String>,
    #[serde(default)]
    pub stages: Option<Vec<StageParam>>,
}

#[derive(Deserialize)]
pub struct StageParam {
    pub agent_id: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Deserialize)]
pub struct QueryArenaParams {
    pub arena_id: String,
}

#[derive(Deserialize)]
pub struct SettleArenaParams {
    pub arena_id: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle `arena.create` — build an ArenaManifest and create a new arena.
pub async fn handle_create(
    request: JsonRpcRequest,
    manager: SharedArenaManager,
) -> JsonRpcResponse {
    let params: CreateArenaParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Build coordination strategy
    let strategy = match params.strategy.as_str() {
        "peer" => {
            let coordinator = params
                .coordinator
                .clone()
                .or_else(|| params.participants.first().cloned())
                .unwrap_or_default();
            CoordinationStrategy::Peer { coordinator }
        }
        "pipeline" => {
            let stages = params
                .stages
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|s| StageSpec {
                    agent_id: s.agent_id.clone(),
                    description: s.description.clone(),
                    depends_on: s.depends_on.clone(),
                })
                .collect();
            CoordinationStrategy::Pipeline { stages }
        }
        other => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Unknown strategy: '{}'. Use 'peer' or 'pipeline'.", other),
            );
        }
    };

    // Build participants list
    let coordinator_id = match &strategy {
        CoordinationStrategy::Peer { coordinator } => Some(coordinator.clone()),
        CoordinationStrategy::Pipeline { .. } => None,
    };

    let participants: Vec<Participant> = params
        .participants
        .iter()
        .map(|id| {
            let role = if coordinator_id.as_deref() == Some(id.as_str()) {
                ParticipantRole::Coordinator
            } else {
                ParticipantRole::Worker
            };
            Participant {
                agent_id: id.clone(),
                role,
                permissions: ArenaPermissions::from_role(role),
            }
        })
        .collect();

    let created_by = coordinator_id
        .clone()
        .or_else(|| params.participants.first().cloned())
        .unwrap_or_default();

    let manifest = ArenaManifest {
        goal: params.goal,
        strategy,
        participants: participants.clone(),
        created_by,
        created_at: Utc::now(),
    };

    let mut mgr = manager.write().unwrap_or_else(|e| e.into_inner());
    match mgr.create_arena(manifest) {
        Ok((arena_id, handles)) => JsonRpcResponse::success(
            request.id,
            json!({
                "arena_id": arena_id.as_str(),
                "participants": handles.len(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle `arena.query` — snapshot arena state for inspection.
pub async fn handle_query(
    request: JsonRpcRequest,
    manager: SharedArenaManager,
) -> JsonRpcResponse {
    let params: QueryArenaParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let arena_id = ArenaId::from_string(&params.arena_id);
    let mgr = manager.read().unwrap_or_else(|e| e.into_inner());

    match mgr.query_arena(&arena_id) {
        Some(snapshot) => JsonRpcResponse::success(request.id, snapshot),
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Arena not found: {}", arena_id),
        ),
    }
}

/// Handle `arena.settle` — settle an arena and return the report.
pub async fn handle_settle(
    request: JsonRpcRequest,
    manager: SharedArenaManager,
) -> JsonRpcResponse {
    let params: SettleArenaParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let arena_id = ArenaId::from_string(&params.arena_id);
    let mut mgr = manager.write().unwrap_or_else(|e| e.into_inner());

    match mgr.settle_with_facts(&arena_id) {
        Ok((report, _facts)) => JsonRpcResponse::success(
            request.id,
            json!({
                "arena_id": report.arena_id.as_str(),
                "facts_persisted": report.facts_persisted,
                "artifacts_archived": report.artifacts_archived,
                "events_cleared": report.events_cleared,
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_manager() -> SharedArenaManager {
        Arc::new(RwLock::new(ArenaManager::new()))
    }

    fn create_request(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id("arena.create", Some(params), json!(1))
    }

    fn query_request(arena_id: &str) -> JsonRpcRequest {
        JsonRpcRequest::with_id(
            "arena.query",
            Some(json!({ "arena_id": arena_id })),
            json!(2),
        )
    }

    fn settle_request(arena_id: &str) -> JsonRpcRequest {
        JsonRpcRequest::with_id(
            "arena.settle",
            Some(json!({ "arena_id": arena_id })),
            json!(3),
        )
    }

    #[tokio::test]
    async fn test_create_arena_handler() {
        let mgr = make_manager();
        let req = create_request(json!({
            "goal": "Write a report",
            "strategy": "peer",
            "participants": ["agent-a", "agent-b"],
            "coordinator": "agent-a"
        }));

        let resp = handle_create(req, mgr).await;
        assert!(resp.is_success(), "Expected success: {:?}", resp.error);

        let result = resp.result.unwrap();
        assert!(result.get("arena_id").is_some());
        assert_eq!(result["participants"], 2);
    }

    #[tokio::test]
    async fn test_create_pipeline_arena() {
        let mgr = make_manager();
        let req = create_request(json!({
            "goal": "Build feature",
            "strategy": "pipeline",
            "participants": ["agent-a", "agent-b"],
            "stages": [
                { "agent_id": "agent-a", "description": "Design", "depends_on": [] },
                { "agent_id": "agent-b", "description": "Implement", "depends_on": ["agent-a"] }
            ]
        }));

        let resp = handle_create(req, mgr).await;
        assert!(resp.is_success(), "Expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn test_query_arena_handler() {
        let mgr = make_manager();

        // Create first
        let create_req = create_request(json!({
            "goal": "Test query",
            "strategy": "peer",
            "participants": ["agent-a", "agent-b"],
            "coordinator": "agent-a"
        }));
        let create_resp = handle_create(create_req, mgr.clone()).await;
        let arena_id = create_resp.result.unwrap()["arena_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Query
        let query_req = query_request(&arena_id);
        let resp = handle_query(query_req, mgr).await;
        assert!(resp.is_success(), "Expected success: {:?}", resp.error);

        let result = resp.result.unwrap();
        assert_eq!(result["goal"], "Test query");
        assert_eq!(result["status"], "Active");
    }

    #[tokio::test]
    async fn test_query_nonexistent_arena() {
        let mgr = make_manager();
        let req = query_request("nonexistent-id");
        let resp = handle_query(req, mgr).await;
        assert!(resp.is_error());
    }

    #[tokio::test]
    async fn test_settle_arena_handler() {
        let mgr = make_manager();

        // Create first
        let create_req = create_request(json!({
            "goal": "Test settle",
            "strategy": "peer",
            "participants": ["agent-a"],
            "coordinator": "agent-a"
        }));
        let create_resp = handle_create(create_req, mgr.clone()).await;
        let arena_id = create_resp.result.unwrap()["arena_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Settle
        let settle_req = settle_request(&arena_id);
        let resp = handle_settle(settle_req, mgr).await;
        assert!(resp.is_success(), "Expected success: {:?}", resp.error);

        let result = resp.result.unwrap();
        assert_eq!(result["arena_id"], arena_id);
        assert_eq!(result["facts_persisted"], 0);
    }

    #[tokio::test]
    async fn test_invalid_params() {
        let mgr = make_manager();

        // Missing required fields
        let req = JsonRpcRequest::with_id("arena.create", Some(json!({})), json!(1));
        let resp = handle_create(req, mgr.clone()).await;
        assert!(resp.is_error());

        // Missing params entirely
        let req = JsonRpcRequest::with_id("arena.create", None, json!(2));
        let resp = handle_create(req, mgr).await;
        assert!(resp.is_error());
    }

    #[tokio::test]
    async fn test_invalid_strategy() {
        let mgr = make_manager();
        let req = create_request(json!({
            "goal": "Test",
            "strategy": "unknown",
            "participants": ["agent-a"]
        }));

        let resp = handle_create(req, mgr).await;
        assert!(resp.is_error());
        let err_msg = resp.error.unwrap().message;
        assert!(err_msg.contains("Unknown strategy"));
    }
}
