//! POST /v1/embeddings — embedding generation endpoint.

use crate::sync_primitives::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::types::{EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, Usage};

/// Maximum number of inputs in a single batch request.
const MAX_BATCH_SIZE: usize = 128;

/// Maximum character length per input string.
const MAX_INPUT_CHARS: usize = 8192;

pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingRequest>,
) -> Result<Json<EmbeddingResponse>, ApiError> {
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

    // Resolve the stable source without retaining a concrete provider.
    let runtime = state
        .memory_runtime
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Embedding provider not configured".into()))?;

    // Normalize input
    let texts = match req.input {
        EmbeddingInput::Single(s) => vec![s],
        EmbeddingInput::Batch(v) => v,
    };

    // Validate batch size
    if texts.len() > MAX_BATCH_SIZE {
        return Err(ApiError::BadRequest(format!(
            "Too many inputs: {} (max {})",
            texts.len(),
            MAX_BATCH_SIZE
        )));
    }
    if texts.is_empty() {
        return Err(ApiError::BadRequest("'input' must not be empty".into()));
    }

    // Validate per-input length
    for (i, text) in texts.iter().enumerate() {
        if text.chars().count() > MAX_INPUT_CHARS {
            return Err(ApiError::BadRequest(format!(
                "Input {i} exceeds maximum length of {MAX_INPUT_CHARS} characters"
            )));
        }
    }

    let batch = runtime
        .embed_batch(texts)
        .await
        .map_err(|e| ApiError::BadGateway(format!("Embedding provider error: {e}")))?
        .ok_or_else(|| ApiError::ServiceUnavailable("Embedding provider not configured".into()))?;

    // Build response
    let data: Vec<EmbeddingData> = batch
        .embeddings
        .into_iter()
        .enumerate()
        .map(|(i, embedding)| EmbeddingData {
            object: "embedding".to_string(),
            index: i as u32,
            embedding,
        })
        .collect();

    Ok(Json(EmbeddingResponse {
        object: "list".to_string(),
        data,
        model: batch.model,
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
    }))
}
