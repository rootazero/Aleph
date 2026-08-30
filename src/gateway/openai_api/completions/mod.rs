//! POST /v1/chat/completions — dual-mode dispatch.

pub mod agent;
pub mod passthrough;

use crate::sync_primitives::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::types::ChatCompletionRequest;

pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    // Auth check
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;
    if let Some(expected) = (state.api_token)() {
        if !crate::security::secret_equal(Some(token), Some(expected.as_str())) {
            return Err(ApiError::Unauthorized("Invalid API key".into()));
        }
    }

    // Validate
    if req.model.is_empty() {
        return Err(ApiError::BadRequest("Missing 'model' field".into()));
    }
    if req.messages.is_empty() {
        return Err(ApiError::BadRequest("'messages' must not be empty".into()));
    }

    // Route by model prefix
    if req.model.starts_with("aleph/") {
        agent::handle(state, &headers, req).await
    } else {
        passthrough::handle(state, req).await
    }
}
