//! `/v1/admin/*` namespace — IPC entry points for CLI commands while
//! the server holds the singleton lock.
//!
//! Mounted on the gateway router under `/v1/admin`; bearer-auth is
//! enforced uniformly via `admin_auth_middleware` against the vault's
//! current shared token (same secret the OpenAI-compat `/v1/*`
//! routes accept, validated with `crate::security::secret_equal`).
//! Spec C scope covers secrets (memory writes go through the existing
//! `remember` tool).

pub mod resume;
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
    /// Session metadata source for `/v1/admin/resume`'s visibility gate. The
    /// same store the JSON-RPC surface uses, so both faces of `agent.resume`
    /// resolve a session key identically.
    pub session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
}

pub fn router(state: AdminApiState) -> Router {
    Router::new()
        .nest("/secrets", secrets::router())
        .nest("/resume", resume::router())
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

/// A throwaway on-disk session store for the admin-router tests.
///
/// Shared by the three `AdminApiState` fixtures rather than repeated: they all
/// already hold a `TempDir`, and the alternative — making `session_store`
/// optional so tests can pass `None` — would let a production boot that forgot
/// to wire it degrade into a permanently-503 resume route with no compile error
/// to catch it.
#[cfg(test)]
pub(crate) fn test_session_store(
    dir: &std::path::Path,
) -> Arc<dyn crate::gateway::session_store::SessionStore> {
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).expect("session dir");
    Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: base,
            ..Default::default()
        })
        .expect("file session store"),
    )
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
            session_store: test_session_store(dir.path()),
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
