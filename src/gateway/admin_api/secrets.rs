//! `/v1/admin/secrets/*` — vault CRUD via IPC.
//!
//! Backed by `SharedTokenManager` (the production vault). All writes
//! go through `store_secret`/`delete_secret`, which in turn hit
//! `VaultIo` (Spec C Task 8) for atomic temp+rename.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::admin_api::AdminApiState;
use crate::secrets::validate_secret_name;

pub fn router() -> Router<AdminApiState> {
    Router::new()
        .route("/", post(create_or_update_secret).get(list_secrets))
        .route("/{key}", get(get_secret).delete(delete_secret))
}

#[derive(Debug, Deserialize)]
pub struct CreateOrUpdateSecretRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecretSummary {
    pub key: String,
}

async fn create_or_update_secret(
    State(state): State<AdminApiState>,
    Json(body): Json<CreateOrUpdateSecretRequest>,
) -> Result<Json<SecretSummary>, (StatusCode, String)> {
    let key = validate_secret_name(&body.key).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    state
        .shared_token
        .store_secret(&key, &body.value)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(SecretSummary { key }))
}

async fn list_secrets(
    State(state): State<AdminApiState>,
) -> Result<Json<Vec<SecretSummary>>, (StatusCode, String)> {
    let names = state
        .shared_token
        .list_secret_names()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        names
            .into_iter()
            .map(|k| SecretSummary { key: k })
            .collect(),
    ))
}

async fn get_secret(
    State(state): State<AdminApiState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let key = validate_secret_name(&key).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    match state.shared_token.get_secret(&key) {
        Ok(Some(secret)) => Ok(Json(serde_json::json!({
            "key": key,
            "value": secret.expose(),
        }))),
        Ok(None) => Err((StatusCode::NOT_FOUND, format!("no secret: {key}"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn delete_secret(
    State(state): State<AdminApiState>,
    Path(key): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let key = validate_secret_name(&key).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let removed = state
        .shared_token
        .delete_secret(&key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, format!("no secret: {key}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn test_app() -> (Router, TempDir) {
        use crate::config::agent_manager::AgentManager;
        use crate::gateway::security::store::SecurityStore;
        use crate::gateway::security::SharedTokenManager;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SecurityStore::in_memory().expect("in-memory store"));
        let mgr = Arc::new(SharedTokenManager::new(store, dir.path().join("vault")));
        // Vault encryption requires a token; generate one before any store_secret.
        mgr.generate_token().expect("seed token");

        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[agents]\n").unwrap();
        let agent_manager = Arc::new(AgentManager::new(
            cfg,
            dir.path().join("workspaces"),
            dir.path().join("agents"),
            dir.path().join("trash"),
        ));

        let state = AdminApiState {
            shared_token: mgr,
            agent_manager,
        };
        let app = Router::new().nest("/secrets", router()).with_state(state);
        (app, dir)
    }

    #[tokio::test]
    async fn round_trip_create_get_list_delete() {
        let (app, _dir) = test_app();
        let body = serde_json::to_vec(&serde_json::json!({
            "key": "OPENAI_API_KEY",
            "value": "sk-test"
        }))
        .unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/secrets")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secrets/OPENAI_API_KEY")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["key"], "OPENAI_API_KEY");
        assert_eq!(payload["value"], "sk-test");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secrets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
        let names: Vec<SecretSummary> = serde_json::from_slice(&bytes).unwrap();
        assert!(names.iter().any(|s| s.key == "OPENAI_API_KEY"));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/secrets/OPENAI_API_KEY")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secrets/OPENAI_API_KEY")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_secret_returns_404() {
        let (app, _dir) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secrets/NOT_THERE")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_secret_names_return_400() {
        let (app, _dir) = test_app();
        let body = serde_json::to_vec(&serde_json::json!({
            "key": "bad name",
            "value": "secret"
        }))
        .unwrap();
        let requests = [
            Request::builder()
                .method("POST")
                .uri("/secrets")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Request::builder()
                .method("GET")
                .uri("/secrets/bad$name")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri("/secrets/bad$name")
                .body(Body::empty())
                .unwrap(),
        ];

        let mut statuses = Vec::new();
        for request in requests {
            statuses.push(
                app.clone()
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
            );
        }
        assert_eq!(statuses, vec![StatusCode::BAD_REQUEST; 3]);
    }
}
