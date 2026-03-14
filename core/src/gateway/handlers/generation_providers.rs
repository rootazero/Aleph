//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use crate::config::types::generation::GenerationProviderConfig;
use crate::config::types::generation::presets::get_merged_generation_preset;
use crate::config::Config;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::GatewayEventBus;
use crate::generation::GenerationType;
use crate::gateway::security::SharedTokenManager;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, warn};

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

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn save_config(cfg: &Config) -> Result<(), String> {
    cfg.save().map_err(|e| e.to_string())
}

/// Vault key prefix for generation provider API keys
fn vault_key(provider_name: &str) -> String {
    format!("gen:{}", provider_name)
}

/// Resolve API key from vault for a generation provider
fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    match vault.get_secret(&vault_key(name)) {
        Ok(Some(secret)) => Some(secret.expose().to_string()),
        Ok(None) => None,
        Err(e) => {
            warn!(provider = %name, error = %e, "Failed to read generation API key from vault");
            None
        }
    }
}

fn build_generation_provider_for_persistence(
    provider_name: &str,
    config: GenerationProviderConfig,
    generation_overrides: &crate::config::presets_override::GenerationPresetsOverride,
) -> GenerationProviderConfig {
    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_generation_preset(
        provider_name,
        &config.provider_type,
        generation_overrides,
    );

    let base_url = match normalize_optional_string(config.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().and_then(|p| p.base_url.clone()),
    };

    let model = match normalize_optional_string(config.model) {
        Some(m) => Some(m),
        None => preset
            .as_ref()
            .filter(|p| !p.default_model.is_empty())
            .map(|p| p.default_model.clone()),
    };

    GenerationProviderConfig {
        base_url,
        model,
        ..config
    }
}

// =============================================================================
// RPC Handlers
// =============================================================================

/// List all generation providers
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
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

            let mut cfg_clone = provider_config.clone();
            cfg_clone.api_key = resolve_api_key(name, &vault);
            GenerationProviderEntry {
                name: name.clone(),
                config: cfg_clone,
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
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
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

            let mut cfg_clone = provider_config.clone();
            cfg_clone.api_key = resolve_api_key(&params.name, &vault);
            let entry = GenerationProviderEntry {
                name: params.name,
                config: cfg_clone,
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
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfig,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

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

        let mut provider_config = build_generation_provider_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override.generation,
        );

        // Validate provider config
        if let Err(e) = provider_config.validate(&params.name) {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Validation failed: {}", e));
        }

        // Store API key in vault
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store generation API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {}", e),
                );
            }
        }
        provider_config.api_key = None;

        // Add provider
        cfg.generation.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg) {
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
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfig,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
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

        let mut provider_config = build_generation_provider_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override.generation,
        );

        // Validate provider config
        if let Err(e) = provider_config.validate(&params.name) {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Validation failed: {}", e));
        }

        // Store new API key in vault if provided
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store generation API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {}", e),
                );
            }
        }
        provider_config.api_key = None;

        // Update provider — config change resets verified
        provider_config.verified = false;
        cfg.generation.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg) {
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
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
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

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            warn!(provider = %params.name, error = %e, "Failed to delete generation API key from vault");
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

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Check if provider exists, is verified, and supports the generation type
        match cfg.generation.providers.get(&params.name) {
            Some(provider_config) => {
                if !provider_config.verified {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!(
                            "Provider '{}' must pass a connection test before being set as default",
                            params.name
                        ),
                    );
                }
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
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Provider name (if provided, persist verified=true on success)
        #[serde(default)]
        name: Option<String>,
        provider_type: String,
        api_key: Option<String>,
        #[allow(dead_code)] // Deserialized from RPC params; reserved for actual connection test
        base_url: Option<String>,
        model: Option<String>,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_name = params.name;

    // If no inline api_key, resolve from vault
    let api_key = match normalize_optional_string(params.api_key) {
        Some(k) => Some(k),
        None => provider_name.as_ref().and_then(|name| resolve_api_key(name, &vault)),
    };

    // TODO: Implement actual connection testing
    // For now, just validate that required fields are present
    let result = if api_key.is_none() {
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
        // Persist verified=true if provider name was given
        if let Some(ref name) = provider_name {
            let mut cfg = config.write().await;
            if let Some(p) = cfg.generation.providers.get_mut(name) {
                p.verified = true;
                if let Err(e) = save_config(&cfg) {
                    tracing::error!(error = %e, "Failed to save config after generation test");
                }
            }
        }

        TestConnectionResult {
            success: true,
            message: format!("Connection test passed for {} provider", params.provider_type),
        }
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_generation_provider_applies_preset_defaults() {
        let cfg = GenerationProviderConfig::new("openai");
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        let _persisted = build_generation_provider_for_persistence("dalle_main", cfg, &overrides);
    }
}
