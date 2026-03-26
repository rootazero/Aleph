//! GET /v1/models handlers (placeholder — implemented in Task 3)

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use super::auth::ApiError;
use super::state::OpenAiApiState;
use super::types::{ModelList, ModelObject};

pub async fn list_models(State(_state): State<Arc<OpenAiApiState>>) -> Json<ModelList> {
    Json(ModelList {
        object: "list".to_string(),
        data: vec![],
    })
}

pub async fn get_model(
    State(_state): State<Arc<OpenAiApiState>>,
    Path(_model_id): Path<String>,
) -> Result<Json<ModelObject>, ApiError> {
    Err(ApiError::NotFound("Not implemented yet".to_string()))
}
