//! `/v1/admin/*` namespace — IPC entry points for CLI commands while
//! the server holds the singleton lock.
//!
//! Mounted on the gateway router under `/v1/admin`; bearer-auth is
//! enforced uniformly via `admin_auth_middleware` against the vault's
//! current shared token (same secret the OpenAI-compat `/v1/*`
//! routes accept, validated with `crate::security::secret_equal`).
//! Spec C scope covers secrets and agents (memory writes go through
//! the existing `remember` tool).

// `/v1/admin/agents` is intentionally NOT mounted: the three handlers
// (`POST /`, `PATCH /{id}`, `DELETE /{id}`) had zero production callers —
// `aleph agent create / update / delete` was never built, and `CommandPolicy`
// cannot dispatch dynamic-path routes through `LockOrIpc`. The module is
// retained so the next iteration re-mounts it as soon as the CLI surfaces
// those commands; see `agents.rs` for the unused handlers.
pub mod agents;
pub mod secrets;

use crate::sync_primitives::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};

use crate::config::agent_manager::AgentManager;
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::security::SharedTokenManager;

#[derive(Clone)]
pub struct AdminApiState {
    pub shared_token: Arc<SharedTokenManager>,
    pub agent_manager: Arc<AgentManager>,
}

pub fn router(state: AdminApiState) -> Router {
    Router::new()
        .nest("/secrets", secrets::router())
        .with_state(state.clone())
        .layer(from_fn_with_state(state, admin_auth_middleware))
}

async fn admin_auth_middleware(
    State(state): State<AdminApiState>,
    request: Request,
    next: Next,
) -> Response {
    let header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let provided = match extract_bearer_token(header) {
        Some(t) => t,
        None => return admin_unauthorized("Missing or invalid Authorization header"),
    };

    let expected = state.shared_token.get_current_token();
    if !crate::security::secret_equal(Some(provided), expected.as_deref()) {
        return admin_unauthorized("Invalid API key");
    }

    next.run(request).await
}

fn admin_unauthorized(message: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "authentication_error",
            "code": "invalid_api_key",
        }
    });
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header::AUTHORIZATION, Request, StatusCode};
    use tower::ServiceExt;

    fn test_app() -> (Router, String, tempfile::TempDir) {
        use crate::gateway::security::store::SecurityStore;

        let dir = tempfile::tempdir().unwrap();
        let shared_token = Arc::new(SharedTokenManager::new(
            Arc::new(SecurityStore::in_memory().unwrap()),
            dir.path().join("vault"),
        ));
        let token = shared_token.generate_token().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "[agents]\n").unwrap();
        let agent_manager = Arc::new(AgentManager::new(
            config_path,
            dir.path().join("workspaces"),
            dir.path().join("agents"),
            dir.path().join("trash"),
        ));
        let app = router(AdminApiState {
            shared_token,
            agent_manager,
        });
        (app, token, dir)
    }

    #[tokio::test]
    async fn admin_routes_require_current_shared_token() {
        let (app, token, _dir) = test_app();

        for authorization in [None, Some("Bearer wrong-token")] {
            let mut request = Request::builder().uri("/secrets");
            if let Some(value) = authorization {
                request = request.header(AUTHORIZATION, value);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/secrets")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
