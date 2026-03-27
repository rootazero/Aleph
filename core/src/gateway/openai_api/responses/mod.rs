//! OpenAI Responses API handler (placeholder) and SSE formatter.

pub mod sse;

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;

use super::auth::ApiError;
use super::state::OpenAiApiState;
use super::types::ResponsesRequest;

/// Placeholder handler for `POST /v1/responses`.
///
/// Returns 503 Service Unavailable until the full Responses API pipeline is wired.
pub async fn handle(
    State(_state): State<Arc<OpenAiApiState>>,
    _headers: HeaderMap,
    Json(_req): Json<ResponsesRequest>,
) -> Result<Response, ApiError> {
    Err(ApiError::ServiceUnavailable(
        "Responses API not yet implemented".into(),
    ))
}
