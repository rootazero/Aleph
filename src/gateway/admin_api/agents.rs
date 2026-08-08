//! `/v1/admin/agents/*` — agent definition CRUD via IPC.
//!
//! Backed by `AgentManager::create/update/delete`. Bodies serialize as
//! the existing `AgentDefinition` (POST) and `AgentPatch` (PATCH) types
//! so handlers are thin pass-throughs without field-mapping drift.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use crate::config::agent_manager::AgentPatch;
use crate::config::types::agents_def::AgentDefinition;
use crate::gateway::admin_api::AdminApiState;

pub fn router() -> Router<AdminApiState> {
    Router::new().route("/", post(create_agent)).route(
        "/{id}",
        axum::routing::patch(update_agent).delete(delete_agent),
    )
}

async fn create_agent(
    State(state): State<AdminApiState>,
    Json(def): Json<AgentDefinition>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let id = def.id.clone();
    state
        .agent_manager
        .create(def)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_agent(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Json(patch): Json<AgentPatch>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .agent_manager
        .update(&id, patch)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_agent(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .agent_manager
        .delete(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::agent_manager::AgentManager;
    use crate::sync_primitives::Arc;
    use axum::body::Body;
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn test_app() -> (Router, TempDir, Arc<AgentManager>) {
        use crate::gateway::security::store::SecurityStore;
        use crate::gateway::security::SharedTokenManager;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "[agents]\n").unwrap();
        let mgr = Arc::new(AgentManager::new(
            config_path,
            dir.path().join("workspaces"),
            dir.path().join("agents"),
            dir.path().join("trash"),
        ));

        let shared = Arc::new(SharedTokenManager::new(
            Arc::new(SecurityStore::in_memory().unwrap()),
            dir.path().join("vault"),
        ));
        shared.generate_token().unwrap();

        let state = AdminApiState {
            shared_token: shared,
            agent_manager: mgr.clone(),
            session_store: crate::gateway::admin_api::test_session_store(dir.path()),
        };
        let app = Router::new().nest("/agents", router()).with_state(state);
        (app, dir, mgr)
    }

    #[tokio::test]
    async fn create_then_delete_agent() {
        let (app, _dir, mgr) = test_app();
        let body = serde_json::json!({
            "id": "alpha",
            "default": false,
            "name": "Alpha"
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agents")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(mgr.get("alpha").is_ok());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/agents/alpha")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
}
