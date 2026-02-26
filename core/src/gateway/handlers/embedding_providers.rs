//! Embedding Providers RPC handlers
//!
//! Provides RPC methods for managing embedding providers (vector embedding services).
//!
//! | Method | Description |
//! |--------|-------------|
//! | embedding_providers.list | List all configured embedding providers |
//! | embedding_providers.get | Get a single provider by id |
//! | embedding_providers.add | Add a new provider config |
//! | embedding_providers.update | Update an existing provider |
//! | embedding_providers.remove | Remove a provider by id |
//! | embedding_providers.setActive | Set the active provider |
//! | embedding_providers.test | Test provider connectivity |
//! | embedding_providers.presets | Return preset configurations |

use crate::config::types::memory::{EmbeddingPreset, EmbeddingProviderConfig};
use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::embedding_provider::RemoteEmbeddingProvider;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

// =============================================================================
// RPC Handlers
// =============================================================================

/// List all configured embedding providers with `is_active` flag.
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let settings = &cfg.memory.embedding;

    let providers: Vec<serde_json::Value> = settings
        .providers
        .iter()
        .map(|p| {
            let mut val = serde_json::to_value(p).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "is_active".to_string(),
                    serde_json::json!(p.id == settings.active_provider_id),
                );
            }
            val
        })
        .collect();

    JsonRpcResponse::success(request.id, serde_json::json!(providers))
}

/// Get a single embedding provider by id.
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    let cfg = config.read().await;
    let settings = &cfg.memory.embedding;

    match settings.providers.iter().find(|p| p.id == params.id) {
        Some(provider) => {
            let mut val = serde_json::to_value(provider).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "is_active".to_string(),
                    serde_json::json!(provider.id == settings.active_provider_id),
                );
            }
            JsonRpcResponse::success(request.id, val)
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Embedding provider '{}' not found", params.id),
        ),
    }
}

/// Add a new embedding provider config (validate id uniqueness).
pub async fn handle_add(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        config: EmbeddingProviderConfig,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    let provider_config = params.config;

    {
        let mut cfg = config.write().await;

        // Check if provider id already exists
        if cfg.memory.embedding.providers.iter().any(|p| p.id == provider_config.id) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Embedding provider '{}' already exists", provider_config.id),
            );
        }

        // Add provider
        cfg.memory.embedding.providers.push(provider_config.clone());

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "added",
        "provider_id": provider_config.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Update an existing embedding provider by id.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        config: EmbeddingProviderConfig,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    {
        let mut cfg = config.write().await;

        // Find and update the provider
        let provider = cfg.memory.embedding.providers.iter_mut().find(|p| p.id == params.id);

        match provider {
            Some(existing) => {
                *existing = params.config;
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Embedding provider '{}' not found", params.id),
                );
            }
        }

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "updated",
        "provider_id": params.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Remove an embedding provider by id (reject if it's the active one).
pub async fn handle_remove(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.memory.embedding.providers.iter().any(|p| p.id == params.id) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Embedding provider '{}' not found", params.id),
            );
        }

        // Reject if it's the active provider
        if cfg.memory.embedding.active_provider_id == params.id {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot remove provider '{}': it is the active embedding provider. Switch to another provider first.",
                    params.id
                ),
            );
        }

        // Remove provider
        cfg.memory.embedding.providers.retain(|p| p.id != params.id);

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "removed",
        "provider_id": params.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Set a provider as active; return `{ "should_clear": true/false }`.
///
/// `should_clear` is true when the active provider changes (dimensions may differ),
/// signaling that the vector store should be rebuilt.
pub async fn handle_set_active(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    let should_clear;

    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.memory.embedding.providers.iter().any(|p| p.id == params.id) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Embedding provider '{}' not found", params.id),
            );
        }

        let old_id = cfg.memory.embedding.active_provider_id.clone();
        should_clear = old_id != params.id;
        cfg.memory.embedding.active_provider_id = params.id.clone();

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "set_active",
        "provider_id": params.id,
        "should_clear": should_clear,
    }));

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "should_clear": should_clear }),
    )
}

/// Test an embedding provider's connectivity.
///
/// Creates a temporary `RemoteEmbeddingProvider` from the provided config and
/// calls `test_connection()` (embeds the word "test" and checks dimension match).
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Either a full config to test, or an id of an existing provider
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        config: Option<EmbeddingProviderConfig>,
    }

    let params: Params = match request.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(params) => params,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
        }
    };

    // Resolve the provider config to test
    let provider_config = if let Some(cfg) = params.config {
        cfg
    } else if let Some(id) = params.id {
        let cfg = config.read().await;
        match cfg.memory.embedding.providers.iter().find(|p| p.id == id) {
            Some(p) => p.clone(),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Embedding provider '{}' not found", id),
                )
            }
        }
    } else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Either 'id' or 'config' must be provided",
        );
    };

    // Create a temporary provider and test connectivity
    let provider = match RemoteEmbeddingProvider::from_config(&provider_config) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "success": false,
                    "message": format!("Failed to create provider: {}", e),
                }),
            )
        }
    };

    match provider.test_connection().await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "success": true,
                "message": format!(
                    "Connection successful — model '{}', {} dimensions",
                    provider_config.model, provider_config.dimensions
                ),
            }),
        ),
        Err(e) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "success": false,
                "message": format!("Connection test failed: {}", e),
            }),
        ),
    }
}

/// Return static list of preset configurations.
pub async fn handle_presets(request: JsonRpcRequest) -> JsonRpcResponse {
    let presets = serde_json::json!([
        {
            "preset": EmbeddingPreset::SiliconFlow,
            "id": "siliconflow",
            "name": "SiliconFlow",
            "api_base": "https://api.siliconflow.cn/v1",
            "api_key_env": "SILICONFLOW_API_KEY",
            "model": "BAAI/bge-m3",
            "dimensions": 1024,
        },
        {
            "preset": EmbeddingPreset::OpenAi,
            "id": "openai",
            "name": "OpenAI",
            "api_base": "https://api.openai.com/v1",
            "api_key_env": "OPENAI_API_KEY",
            "model": "text-embedding-3-small",
            "dimensions": 1536,
        },
        {
            "preset": EmbeddingPreset::Ollama,
            "id": "ollama",
            "name": "Ollama",
            "api_base": "http://localhost:11434/v1",
            "api_key_env": null,
            "model": "nomic-embed-text",
            "dimensions": 768,
        },
    ]);

    JsonRpcResponse::success(request.id, presets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn test_handle_presets() {
        let request = JsonRpcRequest::with_id("embedding_providers.presets", None, serde_json::json!(1));
        let response = handle_presets(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let presets = result.as_array().unwrap();
        assert_eq!(presets.len(), 3);
    }

    #[tokio::test]
    async fn test_handle_list() {
        let config = Arc::new(RwLock::new(Config::default()));
        let request = JsonRpcRequest::with_id("embedding_providers.list", None, serde_json::json!(1));
        let response = handle_list(request, config).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let providers = result.as_array().unwrap();
        // Default config has 3 providers: siliconflow, openai, ollama
        assert_eq!(providers.len(), 3);
        // First provider (siliconflow) should be active
        let first = &providers[0];
        assert_eq!(first["id"].as_str().unwrap(), "siliconflow");
        assert!(first["is_active"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_get_found() {
        let config = Arc::new(RwLock::new(Config::default()));
        let request = JsonRpcRequest::with_id(
            "embedding_providers.get",
            Some(serde_json::json!({ "id": "siliconflow" })),
            serde_json::json!(1),
        );
        let response = handle_get(request, config).await;
        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["id"].as_str().unwrap(), "siliconflow");
        assert!(result["is_active"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_get_not_found() {
        let config = Arc::new(RwLock::new(Config::default()));
        let request = JsonRpcRequest::with_id(
            "embedding_providers.get",
            Some(serde_json::json!({ "id": "nonexistent" })),
            serde_json::json!(1),
        );
        let response = handle_get(request, config).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_remove_rejects_active() {
        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());
        let request = JsonRpcRequest::with_id(
            "embedding_providers.remove",
            Some(serde_json::json!({ "id": "siliconflow" })),
            serde_json::json!(1),
        );
        let response = handle_remove(request, config, event_bus).await;
        assert!(response.is_error());
    }
}
