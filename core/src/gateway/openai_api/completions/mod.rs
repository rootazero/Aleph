//! POST /v1/chat/completions — dual-mode dispatch (placeholder — implemented in Task 5)

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;

use super::auth::ApiError;
use super::state::OpenAiApiState;
use super::types::ChatCompletionRequest;

pub async fn handle(
    State(_state): State<Arc<OpenAiApiState>>,
    _headers: HeaderMap,
    Json(_req): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    Err(ApiError::ServiceUnavailable(
        "Not implemented yet".to_string(),
    ))
}
