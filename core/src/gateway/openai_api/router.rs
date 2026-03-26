//! Axum router for the OpenAI-compatible API.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::auth::ApiError;
use super::state::OpenAiApiState;

pub fn openai_routes(state: Arc<OpenAiApiState>) -> Router {
    Router::new()
        .route("/v1/models", get(super::models::list_models))
        .route("/v1/models/{model_id}", get(super::models::get_model))
        .route("/v1/chat/completions", post(super::completions::handle))
        .route("/v1/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<Arc<OpenAiApiState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "server_id": state.server_id,
    }))
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status =
            StatusCode::from_u16(self.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.to_json())).into_response()
    }
}
