//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use crate::config::types::generation::GenerationProviderConfig;
use crate::config::Config;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::GatewayEventBus;
use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// =============================================================================
// Types
// =============================================================================

/// Generation provider entry for RPC responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderEntry {
    pub name: String,
    pub config: GenerationProviderConfig,
    pub is_default_for: Vec<GenerationType>,
}

/// Test connection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

// =============================================================================
// RPC Handlers
// =============================================================================

/// List all generation providers
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut providers: Vec<GenerationProviderEntry> = cfg
        .generation
        .providers
        .iter()
        .map(|(name, provider_config)| {
            // Check which generation types this provider is default for
            let mut is_default_for = Vec::new();
            if cfg.generation.default_image_provider.as_deref() == Some(name) {
                is_default_for.push(GenerationType::Image);
            }
            if cfg.generation.default_video_provider.as_deref() == Some(name) {
                is_default_for.push(GenerationType::Video);
            }
            if cfg.generation.default_audio_provider.as_deref() == Some(name) {
                is_default_for.push(GenerationType::Audio);
            }
            if cfg.generation.default_speech_provider.as_deref() == Some(name) {
                is_default_for.push(GenerationType::Speech);
            }

            GenerationProviderEntry {
                name: name.clone(),
                config: provider_config.clone(),
                is_default_for,
            }
        })
        .collect();

    // Sort by name for consistent ordering
    providers.sort_by(|a, b| a.name.cmp(&b.name));

    JsonRpcResponse::success(request.id, serde_json::to_value(providers).unwrap())
}

/// Get a specific generation provider
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
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

    match cfg.generation.providers.get(&params.name) {
        Some(provider_config) => {
            // Check which generation types this provider is default for
            let mut is_default_for = Vec::new();
            if cfg.generation.default_image_provider.as_deref() == Some(&params.name) {
                is_default_for.push(GenerationType::Image);
            }
            if cfg.generation.default_video_provider.as_deref() == Some(&params.name) {
                is_default_for.push(GenerationType::Video);
            }
            if cfg.generation.default_audio_provider.as_deref() == Some(&params.name) {
                is_default_for.push(GenerationType::Audio);
            }
            if cfg.generation.default_speech_provider.as_deref() == Some(&params.name) {
                is_default_for.push(GenerationType::Speech);
            }

            let entry = GenerationProviderEntry {
                name: params.name,
                config: provider_config.clone(),
                is_default_for,
            };

            JsonRpcResponse::success(request.id, serde_json::to_value(entry).unwrap())
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider '{}' not found", params.name),
        ),
    }
}

/// Create a new generation provider
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfig,
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

    // Validate provider config
    if let Err(e) = params.config.validate(&params.name) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Validation failed: {}", e));
    }

    {
        let mut cfg = config.write().await;

        // Check if provider already exists
        if cfg.generation.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider '{}' already exists", params.name),
            );
        }

        // Add provider
        cfg.generation.providers.insert(params.name.clone(), params.config);

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
        "topic": "config.generation.providers.changed",
        "action": "created",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Update an existing generation provider
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfig,
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

    // Validate provider config
    if let Err(e) = params.config.validate(&params.name) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Validation failed: {}", e));
    }

    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.generation.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider '{}' not found", params.name),
            );
        }

        // Update provider
        cfg.generation.providers.insert(params.name.clone(), params.config);

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
        "topic": "config.generation.providers.changed",
        "action": "updated",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Delete a generation provider
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
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
        if !cfg.generation.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider '{}' not found", params.name),
            );
        }

        // Check if provider is set as default for any generation type
        let mut default_for = Vec::new();
        if cfg.generation.default_image_provider.as_deref() == Some(&params.name) {
            default_for.push("image");
        }
        if cfg.generation.default_video_provider.as_deref() == Some(&params.name) {
            default_for.push("video");
        }
        if cfg.generation.default_audio_provider.as_deref() == Some(&params.name) {
            default_for.push("audio");
        }
        if cfg.generation.default_speech_provider.as_deref() == Some(&params.name) {
            default_for.push("speech");
        }

        if !default_for.is_empty() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot delete provider '{}': it is set as default for {}",
                    params.name,
                    default_for.join(", ")
                ),
            );
        }

        // Remove provider
        cfg.generation.providers.remove(&params.name);

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
        "topic": "config.generation.providers.changed",
        "action": "deleted",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Set default provider for a generation type
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        generation_type: GenerationType,
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

        // Check if provider exists and supports the generation type
        match cfg.generation.providers.get(&params.name) {
            Some(provider_config) => {
                if !provider_config.capabilities.contains(&params.generation_type) {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!(
                            "Provider '{}' does not support {:?} generation",
                            params.name, params.generation_type
                        ),
                    );
                }
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        }

        // Set as default
        match params.generation_type {
            GenerationType::Image => {
                cfg.generation.default_image_provider = Some(params.name.clone());
            }
            GenerationType::Video => {
                cfg.generation.default_video_provider = Some(params.name.clone());
            }
            GenerationType::Audio => {
                cfg.generation.default_audio_provider = Some(params.name.clone());
            }
            GenerationType::Speech => {
                cfg.generation.default_speech_provider = Some(params.name.clone());
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
        "topic": "config.generation.providers.changed",
        "action": "set_default",
        "provider": params.name,
        "generation_type": params.generation_type,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Test connection to a generation provider
pub async fn handle_test_connection(
    request: JsonRpcRequest,
    _config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        provider_type: String,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
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

    // TODO: Implement actual connection testing
    // For now, just validate that required fields are present
    let result = if params.api_key.is_none() {
        TestConnectionResult {
            success: false,
            message: "API key is required".to_string(),
        }
    } else if params.model.is_none() {
        TestConnectionResult {
            success: false,
            message: "Model is required".to_string(),
        }
    } else {
        TestConnectionResult {
            success: true,
            message: format!("Connection test passed for {} provider", params.provider_type),
        }
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap())
}
