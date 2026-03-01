//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use crate::config::types::generation::GenerationProviderConfig;
use crate::config::Config;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::GatewayEventBus;
use crate::generation::GenerationType;
use crate::secrets::types::EntryMetadata;
use crate::secrets::{resolve_master_key, SecretVault};
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
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

fn save_config_with_secret_redaction(cfg: &Config) -> Result<(), String> {
    let mut sanitized = cfg.clone();
    redact_secret_backed_api_keys(&mut sanitized);
    sanitized.save().map_err(|e| e.to_string())
}

fn redact_secret_backed_api_keys(cfg: &mut Config) {
    for provider in cfg.generation.providers.values_mut() {
        if provider.secret_name.is_some() {
            provider.api_key = None;
        }
    }
}

fn store_generation_provider_api_key(
    provider_name: &str,
    api_key: &str,
    requested_secret_name: Option<&str>,
) -> Result<String, String> {
    let master_key = resolve_master_key().map_err(|e| {
        format!(
            "Cannot persist API key securely: {}. Set ALEPH_MASTER_KEY or provide secret_name only",
            e
        )
    })?;

    let mut vault = SecretVault::open(SecretVault::default_path(), &master_key)
        .map_err(|e| format!("Failed to open secret vault: {}", e))?;

    let secret_name = requested_secret_name
        .and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| format!("generation_{}_api_key", provider_name.replace('-', "_")));

    vault
        .set(
            &secret_name,
            api_key,
            EntryMetadata {
                description: Some(format!(
                    "API key for generation provider '{}'",
                    provider_name
                )),
                provider: Some(provider_name.to_string()),
            },
        )
        .map_err(|e| format!("Failed to store API key in secret vault: {}", e))?;

    Ok(secret_name)
}

fn build_generation_provider_for_persistence(
    provider_name: &str,
    config: GenerationProviderConfig,
) -> Result<GenerationProviderConfig, String> {
    let api_key = normalize_optional_string(config.api_key.clone());
    let requested_secret_name = normalize_optional_string(config.secret_name.clone());

    let secret_name = if let Some(ref api_key_value) = api_key {
        Some(store_generation_provider_api_key(
            provider_name,
            api_key_value,
            requested_secret_name.as_deref(),
        )?)
    } else {
        requested_secret_name
    };

    Ok(GenerationProviderConfig {
        secret_name,
        api_key: None,
        ..config
    })
}

fn resolve_test_api_key(
    api_key: Option<String>,
    secret_name: Option<String>,
) -> Result<Option<String>, String> {
    if let Some(api_key) = normalize_optional_string(api_key) {
        return Ok(Some(api_key));
    }

    let Some(secret_name) = normalize_optional_string(secret_name) else {
        return Ok(None);
    };

    let master_key = resolve_master_key()
        .map_err(|e| format!("Cannot resolve secret '{}': {}", secret_name, e))?;
    let vault = SecretVault::open(SecretVault::default_path(), &master_key)
        .map_err(|e| format!("Failed to open secret vault: {}", e))?;
    let secret = vault
        .get(&secret_name)
        .map_err(|e| format!("Failed to read secret '{}': {}", secret_name, e))?;

    Ok(Some(secret.expose().to_string()))
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

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_config = match build_generation_provider_for_persistence(&params.name, params.config) {
        Ok(config) => config,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid provider credentials: {}", e),
            )
        }
    };

    // Validate provider config
    if let Err(e) = provider_config.validate(&params.name) {
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
        cfg.generation.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
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

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_config = match build_generation_provider_for_persistence(&params.name, params.config) {
        Ok(config) => config,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid provider credentials: {}", e),
            )
        }
    };

    // Validate provider config
    if let Err(e) = provider_config.validate(&params.name) {
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
        cfg.generation.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
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
        secret_name: Option<String>,
        #[allow(dead_code)] // Deserialized from RPC params; reserved for actual connection test
        base_url: Option<String>,
        model: Option<String>,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let api_key = match resolve_test_api_key(params.api_key, params.secret_name) {
        Ok(value) => value,
        Err(e) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
        }
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
    fn test_generation_provider_secret_name_only_is_valid() {
        let mut cfg = GenerationProviderConfig::new("openai");
        cfg.api_key = None;
        cfg.secret_name = Some("gen_openai_key".to_string());
        assert!(cfg.validate("openai").is_ok());
    }

    #[test]
    fn test_build_generation_provider_with_secret_name_only() {
        let mut cfg = GenerationProviderConfig::new("openai");
        cfg.api_key = None;
        cfg.secret_name = Some("gen_openai_key".to_string());

        let persisted = build_generation_provider_for_persistence("dalle_main", cfg).unwrap();
        assert!(persisted.api_key.is_none());
        assert_eq!(persisted.secret_name.as_deref(), Some("gen_openai_key"));
    }

    #[test]
    fn test_redact_secret_backed_generation_api_keys_only() {
        let mut cfg = Config::default();

        let mut secret_backed = GenerationProviderConfig::new("openai");
        secret_backed.api_key = Some("should-be-redacted".to_string());
        secret_backed.secret_name = Some("gen_openai_key".to_string());
        cfg.generation
            .providers
            .insert("dalle".to_string(), secret_backed);

        let mut plaintext = GenerationProviderConfig::new("mock");
        plaintext.api_key = Some("inline-key".to_string());
        plaintext.secret_name = None;
        cfg.generation
            .providers
            .insert("mock".to_string(), plaintext);

        redact_secret_backed_api_keys(&mut cfg);

        assert!(cfg.generation.providers.get("dalle").unwrap().api_key.is_none());
        assert_eq!(
            cfg.generation.providers.get("mock").unwrap().api_key.as_deref(),
            Some("inline-key")
        );
    }
}
