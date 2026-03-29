//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use std::collections::HashMap;
use crate::config::types::generation::GenerationProviderConfig;
use crate::config::types::generation::presets::get_merged_generation_preset;
use crate::config::Config;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::GatewayEventBus;
use crate::generation::GenerationType;
use crate::generation::providers::{OpenAiTtsProvider, ElevenLabsProvider};
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

use super::normalize_optional_string;

fn save_config(cfg: &Config) -> Result<(), String> {
    cfg.save_incremental(&["generation"]).map_err(|e| e.to_string())
}

/// Vault key prefix for generation provider API keys
fn vault_key(provider_name: &str) -> String {
    format!("gen:{}", provider_name)
}

/// Resolve API key from vault for a generation provider
fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(name), vault)
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

    let models = {
        let non_empty: Vec<String> = config.models.iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if non_empty.is_empty() {
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| vec![p.default_model.clone()])
                .unwrap_or_default()
        } else {
            non_empty
        }
    };

    GenerationProviderConfig {
        base_url,
        models,
        ..config
    }
}

// =============================================================================
// Helpers — typed provider map access
// =============================================================================

use crate::config::GenerationConfig;

/// Get an immutable reference to the typed provider map for a given generation type string.
fn get_typed_provider_map<'a>(
    config: &'a GenerationConfig,
    gen_type_str: &str,
) -> &'a HashMap<String, GenerationProviderConfig> {
    match gen_type_str {
        "image" => &config.image_providers,
        "video" => &config.video_providers,
        "speech" => &config.speech_providers,
        "audio" => &config.audio_providers,
        _ => &config.providers, // fallback to legacy
    }
}

/// Get a mutable reference to the typed provider map for a given generation type string.
fn get_typed_provider_map_mut<'a>(
    config: &'a mut GenerationConfig,
    gen_type_str: &str,
) -> &'a mut HashMap<String, GenerationProviderConfig> {
    match gen_type_str {
        "image" => &mut config.image_providers,
        "video" => &mut config.video_providers,
        "speech" => &mut config.speech_providers,
        "audio" => &mut config.audio_providers,
        _ => &mut config.providers,
    }
}

/// Find which typed map contains a provider by name. Returns the type string.
fn find_provider_type(config: &GenerationConfig, name: &str) -> Option<String> {
    if config.image_providers.contains_key(name) {
        return Some("image".to_string());
    }
    if config.video_providers.contains_key(name) {
        return Some("video".to_string());
    }
    if config.speech_providers.contains_key(name) {
        return Some("speech".to_string());
    }
    if config.audio_providers.contains_key(name) {
        return Some("audio".to_string());
    }
    // Legacy: derive from capabilities
    if let Some(cfg) = config.providers.get(name) {
        if let Some(gen_type) = cfg.capabilities.first() {
            return Some(format!("{:?}", gen_type).to_lowercase());
        }
        // Legacy provider with no capabilities — treat as legacy bucket
        return Some("legacy".to_string());
    }
    None
}

/// Check if a provider exists in any map (typed or legacy).
fn provider_exists(config: &GenerationConfig, name: &str) -> bool {
    config.image_providers.contains_key(name)
        || config.video_providers.contains_key(name)
        || config.speech_providers.contains_key(name)
        || config.audio_providers.contains_key(name)
        || config.providers.contains_key(name)
}

/// Parse a generation type string into a GenerationType enum.
fn parse_generation_type(s: &str) -> GenerationType {
    match s {
        "video" => GenerationType::Video,
        "speech" => GenerationType::Speech,
        "audio" => GenerationType::Audio,
        _ => GenerationType::Image,
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

    let mut providers: Vec<(GenerationProviderEntry, GenerationType)> = cfg
        .generation
        .merged_providers()
        .into_iter()
        .map(|(name, provider_config, gen_type)| {
            // Check which generation types this provider is default for
            let mut is_default_for = Vec::new();
            if cfg.generation.default_image_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Image);
            }
            if cfg.generation.default_video_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Video);
            }
            if cfg.generation.default_audio_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Audio);
            }
            if cfg.generation.default_speech_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Speech);
            }

            let mut cfg_clone = provider_config;
            cfg_clone.api_key = resolve_api_key(&name, &vault);
            (GenerationProviderEntry {
                name,
                config: cfg_clone,
                is_default_for,
            }, gen_type)
        })
        .collect();

    // Sort by name for consistent ordering
    providers.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    // Serialize and inject api_key (which is #[serde(skip)] on the config struct)
    // Also inject generation_type field
    let json_arr: Vec<serde_json::Value> = providers
        .iter()
        .map(|(entry, gen_type)| {
            let mut val = serde_json::to_value(entry).unwrap_or_default();
            if let Some(ref key) = entry.config.api_key {
                if let Some(obj) = val.get_mut("config").and_then(|c| c.as_object_mut()) {
                    obj.insert("api_key".into(), serde_json::json!(key));
                }
            }
            // Add generation_type to the response
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "generation_type".into(),
                    serde_json::Value::String(format!("{:?}", gen_type).to_lowercase()),
                );
            }
            val
        })
        .collect();

    JsonRpcResponse::success(request.id, serde_json::json!(json_arr))
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

    // Find provider across all typed maps and legacy
    let found = cfg.generation.merged_providers()
        .into_iter()
        .find(|(name, _, _)| name == &params.name);

    match found {
        Some((name, provider_config, gen_type)) => {
            // Check which generation types this provider is default for
            let mut is_default_for = Vec::new();
            if cfg.generation.default_image_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Image);
            }
            if cfg.generation.default_video_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Video);
            }
            if cfg.generation.default_audio_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Audio);
            }
            if cfg.generation.default_speech_provider.as_deref() == Some(name.as_str()) {
                is_default_for.push(GenerationType::Speech);
            }

            let mut cfg_clone = provider_config;
            cfg_clone.api_key = resolve_api_key(&name, &vault);
            let api_key_copy = cfg_clone.api_key.clone();
            let entry = GenerationProviderEntry {
                name,
                config: cfg_clone,
                is_default_for,
            };

            // Serialize and inject api_key (which is #[serde(skip)] on the config struct)
            let mut val = serde_json::to_value(entry).unwrap_or_default();
            if let Some(ref key) = api_key_copy {
                if let Some(obj) = val.get_mut("config").and_then(|c| c.as_object_mut()) {
                    obj.insert("api_key".into(), serde_json::json!(key));
                }
            }
            // Add generation_type to the response
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "generation_type".into(),
                    serde_json::Value::String(format!("{:?}", gen_type).to_lowercase()),
                );
            }
            JsonRpcResponse::success(request.id, val)
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
        #[serde(default)]
        generation_type: Option<String>,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Determine generation type from explicit param or capabilities
    let gen_type_str = params.generation_type
        .as_deref()
        .or_else(|| params.config.capabilities.first().map(|g| match g {
            GenerationType::Image => "image",
            GenerationType::Video => "video",
            GenerationType::Speech => "speech",
            GenerationType::Audio => "audio",
        }))
        .unwrap_or("image");
    let gen_type = parse_generation_type(gen_type_str);

    {
        let mut cfg = config.write().await;

        // Check if provider already exists in any map
        if provider_exists(&cfg.generation, &params.name) {
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

        // Set capabilities from generation type
        provider_config.capabilities = vec![gen_type];

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

        // Insert into the correct typed map
        get_typed_provider_map_mut(&mut cfg.generation, gen_type_str)
            .insert(params.name.clone(), provider_config);

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

        // Find which typed map contains the provider
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

        let mut provider_config = build_generation_provider_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override.generation,
        );

        // Preserve capabilities from the typed map
        provider_config.capabilities = vec![parse_generation_type(&gen_type_str)];

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
        get_typed_provider_map_mut(&mut cfg.generation, &gen_type_str)
            .insert(params.name.clone(), provider_config);

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

        // Find which typed map contains the provider
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

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

        // Remove provider from the correct typed map
        get_typed_provider_map_mut(&mut cfg.generation, &gen_type_str)
            .remove(&params.name);

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            warn!(provider = %params.name, error = %e, "Failed to delete generation API key from vault");
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["generation"]) {
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

        // Find provider across all typed maps
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

        let provider_config = match get_typed_provider_map(&cfg.generation, &gen_type_str)
            .get(&params.name)
        {
            Some(c) => c,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Provider '{}' disappeared unexpectedly", params.name),
                );
            }
        };

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

        // Validate that the provider's type matches the requested default type
        let provider_gen_type = parse_generation_type(&gen_type_str);
        if provider_gen_type != params.generation_type {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Provider '{}' is a {} provider and cannot be set as default for {:?}",
                    params.name, gen_type_str, params.generation_type
                ),
            );
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
        if let Err(e) = cfg.save_incremental(&["generation"]) {
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
            if let Some(type_str) = find_provider_type(&cfg.generation, name) {
                if let Some(p) = get_typed_provider_map_mut(&mut cfg.generation, &type_str).get_mut(name) {
                    p.verified = true;
                    if let Err(e) = save_config(&cfg) {
                        tracing::error!(error = %e, "Failed to save config after generation test");
                    }
                }
            }
        }

        TestConnectionResult {
            success: true,
            message: format!("Connection test passed for {} provider", params.provider_type),
        }
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to serialize result: {}", e)),
    }
}

/// Get voices for a generation provider
///
/// Returns a list of supported voices for the given provider.
/// Looks up by provider name, then falls back to a static list by provider_type.
pub async fn handle_voices(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        provider_id: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cfg = config.read().await;

    // Determine the provider_type from config (search all typed maps), or treat provider_id as provider_type
    let provider_type = cfg
        .generation
        .merged_providers()
        .iter()
        .find(|(name, _, _)| name == &params.provider_id)
        .map(|(_, cfg, _)| cfg.provider_type.clone())
        .unwrap_or_else(|| params.provider_id.clone())
        .to_lowercase();

    let voices = match provider_type.as_str() {
        "openai" | "openai-tts" | "openai_tts" | "openai_compat" | "openai-compat" => {
            OpenAiTtsProvider::static_voice_list()
        }
        "elevenlabs" => ElevenLabsProvider::static_voice_list(),
        _ => vec![],
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(voices).unwrap_or_default())
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
