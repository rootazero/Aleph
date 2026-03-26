//! GET /v1/models — hybrid model listing.
//!
//! Returns virtual agent IDs (aleph/default, aleph/{agent_id}) plus
//! real model names from all configured providers.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use super::auth::ApiError;
use super::state::OpenAiApiState;
use super::types::{ModelList, ModelObject};

/// Build the full model list (virtual agents + real provider models).
async fn build_model_list(state: &OpenAiApiState) -> Vec<ModelObject> {
    let mut models = Vec::new();
    let mut seen = HashSet::new();

    // 1. Virtual agent IDs
    models.push(ModelObject {
        id: "aleph/default".to_string(),
        object: "model".to_string(),
        created: state.created_at,
        owned_by: "aleph".to_string(),
    });
    seen.insert("aleph/default".to_string());

    if let Some(ref registry) = state.agent_registry {
        for agent_id in registry.list().await {
            let model_id = format!("aleph/{}", agent_id);
            if seen.insert(model_id.clone()) {
                models.push(ModelObject {
                    id: model_id,
                    object: "model".to_string(),
                    created: state.created_at,
                    owned_by: "aleph".to_string(),
                });
            }
        }
    }

    // 2. Real models from provider configs (first occurrence wins for dedup)
    for (provider_name, config) in state.provider_configs.iter() {
        for model_name in &config.models {
            if seen.insert(model_name.clone()) {
                models.push(ModelObject {
                    id: model_name.clone(),
                    object: "model".to_string(),
                    created: state.created_at,
                    owned_by: provider_name.clone(),
                });
            }
        }
    }

    models
}

pub async fn list_models(State(state): State<Arc<OpenAiApiState>>) -> Json<ModelList> {
    let models = build_model_list(&state).await;
    Json(ModelList {
        object: "list".to_string(),
        data: models,
    })
}

pub async fn get_model(
    State(state): State<Arc<OpenAiApiState>>,
    Path(model_id): Path<String>,
) -> Result<Json<ModelObject>, ApiError> {
    let models = build_model_list(&state).await;
    models
        .into_iter()
        .find(|m| m.id == model_id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", model_id)))
}
