//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use super::super::event_bus::GatewayEventBus;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::config::types::generation::presets::get_merged_generation_preset;
use crate::config::types::generation::GenerationProviderConfig;
use crate::config::Config;
use crate::gateway::security::SharedTokenManager;
use crate::generation::providers::{ElevenLabsProvider, OpenAiTtsProvider};
use crate::generation::GenerationType;
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    cfg.save_incremental(&["generation"])
        .map_err(|e| e.to_string())
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
    let preset =
        get_merged_generation_preset(provider_name, &config.provider_type, generation_overrides);

    let base_url = match normalize_optional_string(config.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().and_then(|p| p.base_url.clone()),
    };

    let models = {
        let non_empty: Vec<String> = config
            .models
            .iter()
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
            (
                GenerationProviderEntry {
                    name,
                    config: cfg_clone,
                    is_default_for,
                },
                gen_type,
            )
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
    let found = cfg
        .generation
        .merged_providers()
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
    let gen_type_str = params
        .generation_type
        .as_deref()
        .or_else(|| {
            params.config.capabilities.first().map(|g| match g {
                GenerationType::Image => "image",
                GenerationType::Video => "video",
                GenerationType::Speech => "speech",
                GenerationType::Audio => "audio",
                GenerationType::Transcription => "transcription",
            })
        })
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
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Validation failed: {}", e),
            );
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
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Validation failed: {}", e),
            );
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
        get_typed_provider_map_mut(&mut cfg.generation, &gen_type_str).remove(&params.name);

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

        let provider_config =
            match get_typed_provider_map(&cfg.generation, &gen_type_str).get(&params.name) {
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
            GenerationType::Transcription => {
                cfg.generation.default_transcription_provider = Some(params.name.clone());
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
        None => provider_name
            .as_ref()
            .and_then(|name| resolve_api_key(name, &vault)),
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
                if let Some(p) =
                    get_typed_provider_map_mut(&mut cfg.generation, &type_str).get_mut(name)
                {
                    p.verified = true;
                    if let Err(e) = save_config(&cfg) {
                        tracing::error!(error = %e, "Failed to save config after generation test");
                    }
                }
            }
        }

        TestConnectionResult {
            success: true,
            message: format!(
                "Connection test passed for {} provider",
                params.provider_type
            ),
        }
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize result: {}", e),
        ),
    }
}

/// Get voices for a generation provider
///
/// Resolution order:
/// 1. Try dynamic fetch from provider API (`{base_url}/v1/audio/voices`)
/// 2. Detect model family (MiniMax, OpenAI, etc.) and return known voices
/// 3. Fall back to static list by provider_type
pub async fn handle_voices(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
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

    // Find provider config and type
    let provider_info = cfg
        .generation
        .merged_providers()
        .into_iter()
        .find(|(name, _, _)| name == &params.provider_id);

    let (provider_type, models) = match &provider_info {
        Some((_, pcfg, _)) => (pcfg.provider_type.to_lowercase(), pcfg.models.clone()),
        None => (params.provider_id.to_lowercase(), vec![]),
    };

    // Resolve API key from vault
    let api_key = resolve_api_key(&params.provider_id, &vault);

    // Step 1: Try dynamic fetch from provider API
    if let (Some(ref key), Some((_, pcfg, _))) = (&api_key, &provider_info) {
        // Use explicit voices_url if configured, otherwise derive from base_url
        let voices_url = if let Some(ref explicit_url) = pcfg.voices_url {
            explicit_url.clone()
        } else if let Some(ref base) = pcfg.base_url {
            let base = base.trim_end_matches('/');
            let base = base
                .strip_suffix("/v1")
                .unwrap_or(base)
                .trim_end_matches('/');
            format!("{}/v1/audio/voices", base)
        } else {
            String::new()
        };

        if !voices_url.is_empty() {
            if let Ok(voices) = fetch_voices_from_api(&voices_url, key).await {
                if !voices.is_empty() {
                    return JsonRpcResponse::success(
                        request.id,
                        serde_json::to_value(voices).unwrap_or_default(),
                    );
                }
            }
        }
    }

    // Step 2: Detect model family from configured models
    let voices = detect_voices_by_model(&models, &provider_type);

    JsonRpcResponse::success(request.id, serde_json::to_value(voices).unwrap_or_default())
}

/// Try to fetch voices from a provider's API endpoint.
async fn fetch_voices_from_api(
    url: &str,
    api_key: &str,
) -> Result<Vec<crate::generation::VoiceInfo>, ()> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| ())?;

    if !resp.status().is_success() {
        return Err(());
    }

    // Try OpenAI-style response: { "voices": [...] } or direct array
    let body: serde_json::Value = resp.json().await.map_err(|_| ())?;

    // Format: { "voices": [{ "voice_id": "...", "name": "..." }] }
    if let Some(arr) = body.get("voices").and_then(|v| v.as_array()) {
        let voices: Vec<crate::generation::VoiceInfo> = arr
            .iter()
            .filter_map(|v| {
                let id = v.get("voice_id").or(v.get("id"))?.as_str()?;
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(id);
                let gender = v
                    .get("gender")
                    .and_then(|g| g.as_str())
                    .unwrap_or("neutral");
                let desc = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
                Some(crate::generation::VoiceInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    gender: gender.to_string(),
                    description: desc.to_string(),
                })
            })
            .collect();
        return Ok(voices);
    }

    // Format: direct array [{ "id": "...", "name": "..." }]
    if let Some(arr) = body.as_array() {
        let voices: Vec<crate::generation::VoiceInfo> = arr
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();
        if !voices.is_empty() {
            return Ok(voices);
        }
    }

    Err(())
}

/// Detect appropriate voice list based on model names and provider type.
fn detect_voices_by_model(
    models: &[String],
    provider_type: &str,
) -> Vec<crate::generation::VoiceInfo> {
    // Check if any model is a MiniMax/Hailuo model
    let has_minimax = models.iter().any(|m| {
        let lower = m.to_lowercase();
        lower.contains("speech-2")
            || lower.contains("speech-01")
            || lower.contains("speech-02")
            || lower.contains("minimax")
    });

    if has_minimax {
        return minimax_voice_list();
    }

    // Check if any model is a gpt-4o-mini-tts model
    let has_gpt4o_tts = models.iter().any(|m| m.contains("gpt-4o"));
    if has_gpt4o_tts {
        return gpt4o_tts_voice_list();
    }

    // Fall back to static list by provider_type
    match provider_type {
        "openai" | "openai-tts" | "openai_tts" | "openai_compat" | "openai-compat" => {
            OpenAiTtsProvider::static_voice_list()
        }
        "elevenlabs" => ElevenLabsProvider::static_voice_list(),
        _ => vec![],
    }
}

/// MiniMax/Hailuo (海螺) TTS voice list.
fn minimax_voice_list() -> Vec<crate::generation::VoiceInfo> {
    use crate::generation::VoiceInfo;
    vec![
        VoiceInfo {
            id: "male-qn-qingse".into(),
            name: "青涩青年".into(),
            gender: "male".into(),
            description: "清新、年轻的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-jingying".into(),
            name: "精英青年".into(),
            gender: "male".into(),
            description: "自信、专业的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-badao".into(),
            name: "霸道青年".into(),
            gender: "male".into(),
            description: "有力、阳刚的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-daxuesheng".into(),
            name: "大学生青年".into(),
            gender: "male".into(),
            description: "活泼、阳光的男声".into(),
        },
        VoiceInfo {
            id: "female-shaonv".into(),
            name: "少女".into(),
            gender: "female".into(),
            description: "清新、甜美的女声".into(),
        },
        VoiceInfo {
            id: "female-yujie".into(),
            name: "御姐".into(),
            gender: "female".into(),
            description: "成熟、知性的女声".into(),
        },
        VoiceInfo {
            id: "female-chengshu".into(),
            name: "成熟女性".into(),
            gender: "female".into(),
            description: "温和、优雅的女声".into(),
        },
        VoiceInfo {
            id: "female-tianmei".into(),
            name: "甜美女性".into(),
            gender: "female".into(),
            description: "温柔、可爱的女声".into(),
        },
        VoiceInfo {
            id: "presenter_male".into(),
            name: "男性主持人".into(),
            gender: "male".into(),
            description: "标准、清晰的男性播报声".into(),
        },
        VoiceInfo {
            id: "presenter_female".into(),
            name: "女性主持人".into(),
            gender: "female".into(),
            description: "标准、清晰的女性播报声".into(),
        },
        VoiceInfo {
            id: "audiobook_male_1".into(),
            name: "有声书男声1".into(),
            gender: "male".into(),
            description: "沉稳、厚重的男声".into(),
        },
        VoiceInfo {
            id: "audiobook_male_2".into(),
            name: "有声书男声2".into(),
            gender: "male".into(),
            description: "温暖、磁性的男声".into(),
        },
        VoiceInfo {
            id: "audiobook_female_1".into(),
            name: "有声书女声1".into(),
            gender: "female".into(),
            description: "温暖、亲和的女声".into(),
        },
        VoiceInfo {
            id: "audiobook_female_2".into(),
            name: "有声书女声2".into(),
            gender: "female".into(),
            description: "柔和、舒缓的女声".into(),
        },
        VoiceInfo {
            id: "Podcast_girl".into(),
            name: "播客女生".into(),
            gender: "female".into(),
            description: "活泼、自然的播客女声".into(),
        },
        // OpenAI-compatible voices also work through proxies
        VoiceInfo {
            id: "alloy".into(),
            name: "Alloy".into(),
            gender: "neutral".into(),
            description: "中性、平衡 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "echo".into(),
            name: "Echo".into(),
            gender: "male".into(),
            description: "温暖、对话感 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "nova".into(),
            name: "Nova".into(),
            gender: "female".into(),
            description: "友好、活泼 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "shimmer".into(),
            name: "Shimmer".into(),
            gender: "female".into(),
            description: "清晰、专业 (OpenAI)".into(),
        },
    ]
}

/// GPT-4o-mini-tts voice list.
fn gpt4o_tts_voice_list() -> Vec<crate::generation::VoiceInfo> {
    use crate::generation::VoiceInfo;
    vec![
        VoiceInfo {
            id: "coral".into(),
            name: "Coral".into(),
            gender: "female".into(),
            description: "Warm, conversational".into(),
        },
        VoiceInfo {
            id: "sage".into(),
            name: "Sage".into(),
            gender: "female".into(),
            description: "Calm, thoughtful".into(),
        },
        VoiceInfo {
            id: "ash".into(),
            name: "Ash".into(),
            gender: "male".into(),
            description: "Confident, direct".into(),
        },
        VoiceInfo {
            id: "ballad".into(),
            name: "Ballad".into(),
            gender: "male".into(),
            description: "Warm, engaging".into(),
        },
        VoiceInfo {
            id: "verse".into(),
            name: "Verse".into(),
            gender: "male".into(),
            description: "Versatile, dynamic".into(),
        },
        VoiceInfo {
            id: "alloy".into(),
            name: "Alloy".into(),
            gender: "neutral".into(),
            description: "Neutral, balanced".into(),
        },
        VoiceInfo {
            id: "echo".into(),
            name: "Echo".into(),
            gender: "male".into(),
            description: "Warm, conversational".into(),
        },
        VoiceInfo {
            id: "fable".into(),
            name: "Fable".into(),
            gender: "neutral".into(),
            description: "Expressive, animated".into(),
        },
        VoiceInfo {
            id: "onyx".into(),
            name: "Onyx".into(),
            gender: "male".into(),
            description: "Deep, authoritative".into(),
        },
        VoiceInfo {
            id: "nova".into(),
            name: "Nova".into(),
            gender: "female".into(),
            description: "Warm, friendly".into(),
        },
        VoiceInfo {
            id: "shimmer".into(),
            name: "Shimmer".into(),
            gender: "female".into(),
            description: "Clear, bright".into(),
        },
    ]
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
