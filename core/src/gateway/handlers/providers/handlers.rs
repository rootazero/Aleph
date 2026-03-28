//! Provider RPC handler functions.

use serde_json::json;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, ProviderConfig};
use crate::gateway::security::SharedTokenManager;
use super::parse_params;
use super::types::*;
use super::helpers::*;
use crate::providers::message::UnifiedMessage;
use crate::providers::adapter::RequestPayload;

// ============================================================================
// List
// ============================================================================

/// List all providers
pub async fn handle_list(request: JsonRpcRequest, config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
    let config = config.read().await;
    let default_provider = config.general.default_provider.clone();

    let providers: Vec<ProviderInfo> = config
        .providers
        .iter()
        .map(|(name, cfg)| ProviderInfo {
            name: name.clone(),
            enabled: cfg.enabled,
            model: cfg.models.first().cloned().unwrap_or_default(),
            models: cfg.models.clone(),
            provider_type: Some(cfg.protocol()),
            base_url: cfg.base_url.clone(),
            color: cfg.color.clone(),
            timeout_seconds: cfg.timeout_seconds,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
            has_api_key: has_api_key(name, &vault),
            is_default: default_provider.as_ref() == Some(name),
            verified: cfg.verified,
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "providers": providers }))
}

// ============================================================================
// Get
// ============================================================================

/// Get a single provider
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
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
                model: cfg.models.first().cloned().unwrap_or_default(),
                models: cfg.models.clone(),
                provider_type: Some(cfg.protocol()),
                base_url: cfg.base_url.clone(),
                color: cfg.color.clone(),
                timeout_seconds: cfg.timeout_seconds,
                max_tokens: cfg.max_tokens,
                temperature: cfg.temperature,
                has_api_key: has_api_key(&params.name, &vault),
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

/// Update a provider
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
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

        // Convert JSON config to ProviderConfig
        let mut provider_config = build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
        );

        // Store new API key in vault if provided; otherwise vault retains the old one
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {}", e),
                );
            }
        }
        provider_config.api_key = None;

        // Update provider — config change resets verified status
        provider_config.verified = false;
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg, &["providers"]) {
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

/// Create a new provider
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
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

        // Convert JSON config to ProviderConfig
        let mut provider_config = build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
        );

        // Store API key in vault (then clear from config so it's never persisted)
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {}", e),
                );
            }
        }
        provider_config.api_key = None;

        // Insert provider
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg, &["providers"]) {
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

/// Delete a provider
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
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

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            tracing::warn!(provider = %params.name, error = %e, "Failed to delete API key from vault");
        }

        // Save to file
        if let Err(e) = save_config(&cfg, &["providers"]) {
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

/// Test a provider connection (without health reset)
pub async fn handle_test_no_registry(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    handle_test_inner(request, config_store, vault, None).await
}

/// Test a provider connection (with health reset on success)
pub async fn handle_test(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
) -> JsonRpcResponse {
    handle_test_inner(request, config_store, vault, Some(&multi_registry)).await
}

/// Shared implementation for testing a provider connection
async fn handle_test_inner(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Option<&Arc<crate::thinker::MultiProviderRegistry>>,
) -> JsonRpcResponse {
    let params: TestParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_name = params.name;
    let config = params.config;

    // If no api_key in request, fall back to vault
    let effective_api_key = {
        let inline_key = normalize_optional_string(config.api_key.clone());
        if inline_key.is_none() {
            if let Some(ref name) = provider_name {
                resolve_api_key(name, &vault)
            } else {
                None
            }
        } else {
            inline_key
        }
    };

    // Convert JSON config to runtime ProviderConfig
    let provider_config = ProviderConfig {
        protocol: config.protocol,
        api_key: effective_api_key,
        models: config.models.clone(),
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
        model_behavior: None,
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
    let ping_msgs = [UnifiedMessage::user("ping")];
    match provider.process(RequestPayload::new(&ping_msgs).with_system(Some("Reply with 'pong'"))).await {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            // Persist verified=true if provider name was given
            if let Some(ref name) = provider_name {
                let mut cfg = config_store.write().await;
                if let Some(p) = cfg.providers.get_mut(name) {
                    p.verified = true;
                    if let Err(e) = save_config(&cfg, &["providers"]) {
                        error!(error = %e, "Failed to save config after test");
                    }
                }
                // Reset health to Healthy after successful connection test
                if let Some(registry) = multi_registry {
                    use crate::thinker::ProviderRegistry;
                    registry.reset_health(name);
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
// Needs Setup
// ============================================================================

/// Check if first-run setup is needed
///
/// Returns true if no provider is both enabled and verified.
/// Panel calls this on startup to decide whether to show the setup wizard.
pub async fn handle_needs_setup(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config_store.read().await;
    let provider_count = cfg.providers.len();
    let has_verified = cfg.providers.values().any(|p| p.enabled && p.verified);

    JsonRpcResponse::success(
        request.id,
        json!({
            "needs_setup": !has_verified,
            "provider_count": provider_count,
            "has_verified": has_verified,
        }),
    )
}

// ============================================================================
// Set Default
// ============================================================================

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
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    set_default_provider_inner(&request, &params, &config, &event_bus, Some(&multi_registry)).await
}

/// Shared implementation for setting the default provider
async fn set_default_provider_inner(
    request: &JsonRpcRequest,
    params: &SetDefaultParams,
    config: &Arc<RwLock<Config>>,
    event_bus: &Arc<GatewayEventBus>,
    multi_registry: Option<&Arc<crate::thinker::MultiProviderRegistry>>,
) -> JsonRpcResponse {
    // Resolve canonical name (e.g. "codex" → "chatgpt")
    let name = match params.name.to_lowercase().as_str() {
        "codex" => "chatgpt".to_string(),
        _ => params.name.clone(),
    };

    // Set default provider and build new provider instance
    let provider_config_for_swap: Option<(String, crate::config::ProviderConfig)>;
    {
        let mut cfg = config.write().await;

        // Guard: provider must exist and be verified
        match cfg.providers.get(&name) {
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Provider not found: {}", name),
                );
            }
            Some(provider) if !provider.verified => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Provider '{}' must pass a connection test before being set as default", name),
                );
            }
            _ => {}
        }

        // Capture provider config before setting default (for runtime swap)
        provider_config_for_swap = if multi_registry.is_some() {
            cfg.providers.get(&name).map(|pc| (name.clone(), pc.clone()))
        } else {
            None
        };

        // Use the existing set_default_provider method
        if let Err(e) = cfg.set_default_provider(&name) {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Failed to set default provider: {}", e),
            );
        }

        // Save to file (redact resolved secrets before write)
        if let Err(e) = save_config(&cfg, &["general"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id.clone(),
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Hot-swap the runtime provider using MultiProviderRegistry
    if let (Some(registry), Some((name, provider_config))) = (multi_registry, provider_config_for_swap) {
        match crate::providers::create_provider(&name, provider_config) {
            Ok(new_provider) => {
                registry.register(name.clone(), new_provider);
                if let Err(e) = registry.set_default(&name) {
                    error!(name = %name, error = %e, "Failed to set default in multi-registry");
                } else {
                    // Reset health to Healthy when provider is set as default
                    use crate::thinker::ProviderRegistry;
                    registry.reset_health(&name);
                    info!(name = %name, providers = ?registry.list_providers(), "Provider set as default in multi-registry");
                }
            }
            Err(e) => {
                error!(name = %name, error = %e, "Failed to create provider for hot-swap (config saved)");
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
