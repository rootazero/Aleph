//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, create, update, delete, test, setDefault.

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, ProviderConfig};
use crate::config::presets_override::PresetsOverride;
use crate::providers::presets::get_merged_preset;
use crate::secrets::types::EntryMetadata;
use crate::secrets::{resolve_master_key, SecretVault};

/// Provider info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub color: String,
    pub timeout_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub is_default: bool,
    pub verified: bool,
}

/// Test result
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

// ============================================================================
// List
// ============================================================================

/// List all providers
pub async fn handle_list(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let config = config.read().await;
    let default_provider = config.general.default_provider.clone();

    let providers: Vec<ProviderInfo> = config
        .providers
        .iter()
        .map(|(name, cfg)| ProviderInfo {
            name: name.clone(),
            enabled: cfg.enabled,
            model: cfg.model.clone(),
            provider_type: Some(cfg.protocol()),
            base_url: cfg.base_url.clone(),
            color: cfg.color.clone(),
            timeout_seconds: cfg.timeout_seconds,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
            api_key: cfg.api_key.clone(),
            is_default: default_provider.as_ref() == Some(name),
            verified: cfg.verified,
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "providers": providers }))
}

// ============================================================================
// Get
// ============================================================================

/// Parameters for providers.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub name: String,
}

/// Get a single provider
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let config = config.read().await;
    match config.providers.get(&params.name) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let info = ProviderInfo {
                name: params.name.clone(),
                enabled: cfg.enabled,
                model: cfg.model.clone(),
                provider_type: Some(cfg.protocol()),
                base_url: cfg.base_url.clone(),
                color: cfg.color.clone(),
                timeout_seconds: cfg.timeout_seconds,
                max_tokens: cfg.max_tokens,
                temperature: cfg.temperature,
                api_key: cfg.api_key.clone(),
                is_default: default_provider.as_ref() == Some(&params.name),
                verified: cfg.verified,
            };
            JsonRpcResponse::success(request.id, json!({ "provider": info }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.name),
        ),
    }
}

// ============================================================================
// Update
// ============================================================================

/// Parameters for providers.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Provider config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigJson {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub secret_name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub top_k: Option<u32>,
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
    for provider in cfg.providers.values_mut() {
        if provider.secret_name.is_some() {
            provider.api_key = None;
        }
    }
}

fn store_provider_api_key(
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
        .unwrap_or_else(|| format!("{}_api_key", provider_name.replace('-', "_")));

    vault
        .set(
            &secret_name,
            api_key,
            EntryMetadata {
                description: Some(format!("API key for provider '{}'", provider_name)),
                provider: Some(provider_name.to_string()),
            },
        )
        .map_err(|e| format!("Failed to store API key in secret vault: {}", e))?;

    Ok(secret_name)
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

fn build_provider_config_for_persistence(
    provider_name: &str,
    params: ProviderConfigJson,
    presets_override: &PresetsOverride,
) -> Result<ProviderConfig, String> {
    let api_key = normalize_optional_string(params.api_key);
    let requested_secret_name = normalize_optional_string(params.secret_name);

    // Only use SecretVault when user explicitly provides a secret_name.
    // Otherwise store api_key directly in config.toml (plaintext).
    let (persisted_api_key, secret_name) = if let Some(ref sn) = requested_secret_name {
        // User wants encrypted storage — requires ALEPH_MASTER_KEY
        if let Some(ref api_key_value) = api_key {
            let stored_name = store_provider_api_key(
                provider_name,
                api_key_value,
                Some(sn.as_str()),
            )?;
            (None, Some(stored_name))
        } else {
            (None, Some(sn.clone()))
        }
    } else {
        // No secret_name — store api_key directly in config
        (api_key, None)
    };

    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_preset(provider_name, presets_override);

    let base_url = match normalize_optional_string(params.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().map(|p| p.base_url.clone()),
    };

    let model = {
        let trimmed = params.model.trim();
        if trimmed.is_empty() {
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| p.default_model.clone())
                .unwrap_or_default()
        } else {
            trimmed.to_string()
        }
    };

    Ok(ProviderConfig {
        protocol: params.protocol,
        api_key: persisted_api_key,
        secret_name,
        model,
        base_url,
        color: params.color.unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: params.timeout_seconds.unwrap_or(300),
        enabled: params.enabled,
        max_tokens: params.max_tokens,
        temperature: params.temperature,
        top_p: params.top_p,
        top_k: params.top_k,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
        verified: false,
    })
}

/// Update a provider
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Update config
    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.name),
            );
        }

        // Convert JSON config to ProviderConfig and move plaintext api_key into vault
        let mut provider_config = match build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
        ) {
            Ok(cfg) => cfg,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid provider credentials: {}", e),
                );
            }
        };

        // If no new credentials were provided, preserve the existing ones
        if provider_config.api_key.is_none() && provider_config.secret_name.is_none() {
            if let Some(existing) = cfg.providers.get(&params.name) {
                provider_config.api_key = existing.api_key.clone();
                provider_config.secret_name = existing.secret_name.clone();
            }
        }

        // Update provider — config change resets verified status
        provider_config.verified = false;
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "updated", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Create
// ============================================================================

/// Parameters for providers.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Create a new provider
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Create provider
    {
        let mut cfg = config.write().await;

        // Check if provider already exists
        if cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider already exists: {}", params.name),
            );
        }

        // Convert JSON config to ProviderConfig and move plaintext api_key into vault
        let provider_config = match build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
        ) {
            Ok(cfg) => cfg,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid provider credentials: {}", e),
                );
            }
        };

        // Insert provider
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file (redact resolved secrets before write)
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "created", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for providers.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

/// Delete a provider
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Delete provider
    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.name),
            );
        }

        // Check if it's the default provider
        if cfg.general.default_provider.as_ref() == Some(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Cannot delete default provider: {}", params.name),
            );
        }

        // Remove provider
        cfg.providers.remove(&params.name);

        // Save to file (redact resolved secrets before write)
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "deleted", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Test
// ============================================================================

/// Parameters for providers.test
#[derive(Debug, Deserialize)]
pub struct TestParams {
    /// Provider name (if provided, persist verified=true on success)
    #[serde(default)]
    pub name: Option<String>,
    pub config: ProviderConfigJson,
}

/// Test a provider connection
pub async fn handle_test(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let params: TestParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_name = params.name;
    let config = params.config;

    // If no api_key/secret_name in request, fall back to stored credentials
    let (effective_api_key, effective_secret_name) = {
        let inline_key = normalize_optional_string(config.api_key.clone());
        let inline_secret = normalize_optional_string(config.secret_name.clone());
        if inline_key.is_none() && inline_secret.is_none() {
            if let Some(ref name) = provider_name {
                let cfg = config_store.read().await;
                if let Some(existing) = cfg.providers.get(name) {
                    (existing.api_key.clone(), existing.secret_name.clone())
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (inline_key, inline_secret)
        }
    };

    let test_api_key = match resolve_test_api_key(effective_api_key, effective_secret_name) {
        Ok(value) => value,
        Err(e) => {
            return JsonRpcResponse::success(
                request.id,
                json!(TestResult {
                    success: false,
                    error: Some(e),
                    latency_ms: None,
                }),
            );
        }
    };

    // Convert JSON config to runtime ProviderConfig
    let provider_config = ProviderConfig {
        protocol: config.protocol,
        api_key: test_api_key,
        secret_name: normalize_optional_string(config.secret_name),
        model: config.model,
        base_url: config.base_url,
        color: config.color.unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: config.timeout_seconds.unwrap_or(300),
        enabled: config.enabled,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
        verified: false,
    };

    // Create provider instance
    let provider = match crate::providers::create_provider("test", provider_config) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::success(
                request.id,
                json!(TestResult {
                    success: false,
                    error: Some(format!("Failed to create provider: {}", e)),
                    latency_ms: None,
                }),
            );
        }
    };

    // Test the connection with a simple ping
    let start = std::time::Instant::now();
    match provider.process("ping", Some("Reply with 'pong'")).await {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            // Persist verified=true if provider name was given
            if let Some(ref name) = provider_name {
                let mut cfg = config_store.write().await;
                if let Some(p) = cfg.providers.get_mut(name) {
                    p.verified = true;
                    if let Err(e) = save_config_with_secret_redaction(&cfg) {
                        error!(error = %e, "Failed to save config after test");
                    }
                }
            }

            JsonRpcResponse::success(
                request.id,
                json!(TestResult {
                    success: true,
                    error: None,
                    latency_ms: Some(latency_ms),
                }),
            )
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            JsonRpcResponse::success(
                request.id,
                json!(TestResult {
                    success: false,
                    error: Some(format!("{}", e)),
                    latency_ms: Some(latency_ms),
                }),
            )
        }
    }
}

// ============================================================================
// Set Default
// ============================================================================

/// Parameters for providers.setDefault
#[derive(Debug, Deserialize)]
pub struct SetDefaultParams {
    pub name: String,
}

/// Set the default provider (config-only, no runtime swap)
pub async fn handle_set_default_config_only(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    set_default_provider_inner(&request, &params, &config, &event_bus, None).await
}

/// Set the default provider with runtime hot-swap
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    swappable_registry: Arc<crate::thinker::SwappableProviderRegistry>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    set_default_provider_inner(&request, &params, &config, &event_bus, Some(&swappable_registry)).await
}

/// Shared implementation for setting the default provider
async fn set_default_provider_inner(
    request: &JsonRpcRequest,
    params: &SetDefaultParams,
    config: &Arc<RwLock<Config>>,
    event_bus: &Arc<GatewayEventBus>,
    swappable_registry: Option<&Arc<crate::thinker::SwappableProviderRegistry>>,
) -> JsonRpcResponse {
    // Set default provider and build new provider instance
    let provider_config_for_swap: Option<(String, crate::config::ProviderConfig)>;
    {
        let mut cfg = config.write().await;

        // Guard: only verified providers can be set as default
        if let Some(provider) = cfg.providers.get(&params.name) {
            if !provider.verified {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Provider '{}' must pass a connection test before being set as default", params.name),
                );
            }
        }

        // Capture provider config before setting default (for runtime swap)
        provider_config_for_swap = if swappable_registry.is_some() {
            cfg.providers.get(&params.name).map(|pc| (params.name.clone(), pc.clone()))
        } else {
            None
        };

        // Use the existing set_default_provider method
        if let Err(e) = cfg.set_default_provider(&params.name) {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Failed to set default provider: {}", e),
            );
        }

        // Save to file (redact resolved secrets before write)
        if let Err(e) = save_config_with_secret_redaction(&cfg) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id.clone(),
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Hot-swap the runtime provider
    if let (Some(registry), Some((name, provider_config))) = (swappable_registry, provider_config_for_swap) {
        match crate::providers::create_provider(&name, provider_config) {
            Ok(new_provider) => {
                registry.swap(new_provider);
                info!(name = %name, "Runtime provider hot-swapped");
            }
            Err(e) => {
                // Config was saved but runtime swap failed — log but don't fail the request
                error!(name = %name, error = %e, "Failed to hot-swap runtime provider (config saved)");
            }
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "set_default", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Default provider set");
    JsonRpcResponse::success(request.id.clone(), json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn test_update_params() {
        let json = json!({
            "name": "openai",
            "config": {
                "enabled": true,
                "model": "gpt-4"
            }
        });
        let params: UpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "openai");
        assert_eq!(params.config.model, "gpt-4");
    }

    #[test]
    fn test_test_result_serialize() {
        let result = TestResult {
            success: true,
            error: None,
            latency_ms: Some(150),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["latency_ms"], 150);
    }

    #[test]
    fn test_provider_config_json_supports_secret_name() {
        let json = json!({
            "protocol": "openai",
            "enabled": true,
            "model": "gpt-4o",
            "secret_name": "openai_main_api_key"
        });

        let config: ProviderConfigJson = serde_json::from_value(json).unwrap();
        assert_eq!(config.secret_name.as_deref(), Some("openai_main_api_key"));
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_build_provider_config_with_secret_name_only() {
        let params = ProviderConfigJson {
            protocol: Some("openai".to_string()),
            enabled: true,
            model: "gpt-4o".to_string(),
            api_key: None,
            secret_name: Some("openai_main_api_key".to_string()),
            base_url: None,
            color: None,
            timeout_seconds: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };

        let overrides = crate::config::presets_override::PresetsOverride::default();
        let config = build_provider_config_for_persistence("openai-main", params, &overrides).unwrap();
        assert_eq!(config.secret_name.as_deref(), Some("openai_main_api_key"));
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_redact_secret_backed_api_keys_only() {
        let mut config = Config::default();

        let mut secret_backed = ProviderConfig::test_config("gpt-4o");
        secret_backed.api_key = Some("should-be-redacted".to_string());
        secret_backed.secret_name = Some("openai_main_api_key".to_string());
        config.providers.insert("openai".to_string(), secret_backed);

        let mut plaintext = ProviderConfig::test_config("llama3.2");
        plaintext.secret_name = None;
        plaintext.api_key = Some("local-key".to_string());
        config.providers.insert("ollama".to_string(), plaintext);

        redact_secret_backed_api_keys(&mut config);

        assert!(config.providers.get("openai").unwrap().api_key.is_none());
        assert_eq!(
            config.providers.get("ollama").unwrap().api_key.as_deref(),
            Some("local-key")
        );
    }

    #[test]
    fn test_resolve_test_api_key_prefers_inline_key() {
        let key = resolve_test_api_key(
            Some("  sk-inline-test  ".to_string()),
            Some("unused_secret".to_string()),
        )
        .unwrap();

        assert_eq!(key.as_deref(), Some("sk-inline-test"));
    }
}
