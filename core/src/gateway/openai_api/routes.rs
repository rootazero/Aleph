//! Axum router and handlers for the OpenAI-compatible API.
//!
//! Provides `/v1/models`, `/v1/health`, and `/v1/chat/completions` endpoints
//! that mirror the OpenAI API surface. Third-party clients (e.g. Cursor, Copilot)
//! can connect to Aleph through this standard interface.

use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{extract::State, Json, Router};
use serde_json::json;

use super::auth::{extract_bearer_token, ApiError};
use super::types::{
    ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ModelList, Usage,
};

/// Shared state for the OpenAI-compatible API routes.
#[derive(Debug, Clone)]
pub struct OpenAiApiState {
    /// Server identifier surfaced in health checks.
    pub server_id: String,
}

/// Build an axum [`Router`] exposing the OpenAI-compatible API.
///
/// The returned router defines these routes:
///
/// | Method | Path                      | Description              |
/// |--------|---------------------------|--------------------------|
/// | GET    | `/v1/models`              | List available models    |
/// | GET    | `/v1/health`              | Health / readiness probe |
/// | POST   | `/v1/chat/completions`    | Chat completions (stub)  |
pub fn openai_routes(state: Arc<OpenAiApiState>) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/health", get(health))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /v1/models — returns an empty model list for now.
async fn list_models() -> Json<ModelList> {
    Json(ModelList {
        object: "list".to_string(),
        data: vec![],
    })
}

/// GET /v1/health — lightweight readiness probe.
async fn health(State(state): State<Arc<OpenAiApiState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "server_id": state.server_id,
    }))
}

/// POST /v1/chat/completions — stub that validates auth and returns a canned response.
async fn chat_completions(
    State(_state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, ApiError> {
    // --- Auth check ---
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if extract_bearer_token(auth_header).is_none() {
        return Err(ApiError::Unauthorized(
            "Missing or invalid Authorization header".to_string(),
        ));
    }

    // --- Stub response ---
    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: req.model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(
                    "OpenAI API endpoint is ready. Full agent integration pending.".to_string(),
                ),
                tool_calls: None,
            },
            finish_reason: Some("stop".to_string()),
            delta: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }),
    };

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// IntoResponse for ApiError
// ---------------------------------------------------------------------------

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status =
            StatusCode::from_u16(self.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.to_json())).into_response()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state() -> Arc<OpenAiApiState> {
        Arc::new(OpenAiApiState {
            server_id: "test-server".to_string(),
        })
    }

    fn test_app() -> Router {
        openai_routes(test_state())
    }

    #[tokio::test]
    async fn test_list_models_returns_empty_list() {
        let app = test_app();
        let req = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["object"], "list");
        assert_eq!(json["data"], json!([]));
    }

    #[tokio::test]
    async fn test_health_returns_ok() {
        let app = test_app();
        let req = Request::builder()
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["server_id"], "test-server");
    }

    #[tokio::test]
    async fn test_chat_completions_rejects_missing_auth() {
        let app = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&json!({
                    "model": "gpt-4",
                    "messages": [{"role": "user", "content": "Hello"}]
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["type"], "authentication_error");
    }

    #[tokio::test]
    async fn test_chat_completions_returns_stub_with_valid_auth() {
        let app = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", "Bearer sk-test-key")
            .body(Body::from(
                serde_json::to_string(&json!({
                    "model": "gpt-4",
                    "messages": [{"role": "user", "content": "Hello"}]
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["model"], "gpt-4");
        assert!(json["id"].as_str().unwrap().starts_with("chatcmpl-"));
        assert_eq!(json["choices"][0]["message"]["role"], "assistant");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        assert!(json["usage"].is_object());
    }
}
