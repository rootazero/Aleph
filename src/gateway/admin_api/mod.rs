//! `/v1/admin/*` namespace — IPC entry points for CLI commands while
//! the server holds the singleton lock.

pub mod agents;
pub mod secrets;

use crate::sync_primitives::Arc;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;

use crate::config::agent_manager::AgentManager;
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::security::SharedTokenManager;

#[derive(Clone)]
pub struct AdminApiState {
    pub shared_token: Arc<SharedTokenManager>,
    pub agent_manager: Arc<AgentManager>,
}

pub fn router(state: AdminApiState) -> Router {
    let shared_token = state.shared_token.clone();
    Router::new()
        .nest("/secrets", secrets::router())
        .nest("/agents", agents::router())
        .route_layer(middleware::from_fn_with_state(
            shared_token,
            require_shared_token,
        ))
        .with_state(state)
}

async fn require_shared_token(
    State(shared_token): State<Arc<SharedTokenManager>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token);
    let expected = shared_token.get_current_token();
    if !crate::security::secret_equal(provided, expected.as_deref()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
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
