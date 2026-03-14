//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, create, update, delete, test, setDefault.

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, ProviderConfig};
use crate::config::presets_override::PresetsOverride;
use crate::providers::presets::get_merged_preset;
use crate::gateway::security::SharedTokenManager;

/// Vault key prefix for AI provider API keys
fn vault_key(provider_name: &str) -> String {
    format!("ai:{}", provider_name)
}

/// Resolve API key from vault into a ProviderInfo response
fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    match vault.get_secret(&vault_key(name)) {
        Ok(Some(secret)) => Some(secret.expose().to_string()),
        Ok(None) => None,
        Err(e) => {
            warn!(provider = %name, error = %e, "Failed to read API key from vault");
            None
        }
    }
}

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
pub async fn handle_list(request: JsonRpcRequest, config: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
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
            api_key: resolve_api_key(name, &vault),
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
                model: cfg.model.clone(),
                provider_type: Some(cfg.protocol()),
                base_url: cfg.base_url.clone(),
                color: cfg.color.clone(),
                timeout_seconds: cfg.timeout_seconds,
                max_tokens: cfg.max_tokens,
                temperature: cfg.temperature,
                api_key: resolve_api_key(&params.name, &vault),
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

fn save_config(cfg: &Config) -> Result<(), String> {
    // api_key is #[serde(skip)] — never persisted to disk
    cfg.save().map_err(|e| e.to_string())
}


fn build_provider_config_for_persistence(
    provider_name: &str,
    params: ProviderConfigJson,
    presets_override: &PresetsOverride,
) -> ProviderConfig {
    // api_key is runtime-only (#[serde(skip)]) — kept in memory, never persisted
    let api_key = normalize_optional_string(params.api_key);

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

    ProviderConfig {
        protocol: params.protocol,
        api_key,
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
    }
}

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
        if let Err(e) = save_config(&cfg) {
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
        if let Err(e) = save_config(&cfg) {
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
            warn!(provider = %params.name, error = %e, "Failed to delete API key from vault");
        }

        // Save to file
        if let Err(e) = save_config(&cfg) {
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
pub async fn handle_test(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
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
                    if let Err(e) = save_config(&cfg) {
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
// Probe
// ============================================================================

/// Parameters for providers.probe
#[derive(Debug, Deserialize)]
pub struct ProbeParams {
    /// Protocol type: "openai", "anthropic", "gemini", "ollama"
    pub protocol: String,
    /// Provider name — used to resolve API key from vault when api_key is not provided
    #[serde(default)]
    pub name: Option<String>,
    /// API key (not needed for Ollama; resolved from vault if omitted)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Custom base URL (None = protocol default)
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Probe result combining connection test + model discovery
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub models: Vec<crate::providers::adapter::DiscoveredModel>,
    pub model_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Probe a provider: test connection + discover available models
///
/// Combines connection verification and model discovery in a single call.
/// Used by the setup wizard and enhanced settings form.
pub async fn handle_probe(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
    let params: ProbeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let protocol = params.protocol.clone();

    // Build temporary config for probing
    let mut probe_config = ProviderConfig::test_config("probe-placeholder");
    probe_config.protocol = Some(protocol.clone());

    // Resolve API key: explicit param > vault > existing config
    let api_key = params.api_key.or_else(|| {
        params.name.as_deref().and_then(|name| {
            let key = resolve_api_key(name, &vault);
            info!(provider = %name, has_key = key.is_some(), "Probe: resolved API key from vault");
            key
        })
    });
    if let Some(ref key) = api_key {
        info!(protocol = %protocol, key_len = key.len(), "Probe: using API key");
        probe_config.api_key = Some(key.clone());
    } else {
        info!(protocol = %protocol, "Probe: no API key available");
    }

    // Resolve base_url: explicit param > existing config
    let base_url = if params.base_url.is_some() {
        params.base_url
    } else if let Some(name) = params.name.as_deref() {
        let config = config_store.read().await;
        config.providers.get(name).and_then(|c| c.base_url.clone())
    } else {
        None
    };
    if let Some(url) = base_url {
        probe_config.base_url = Some(url);
    }

    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let probe_name = format!("probe-{}", uuid::Uuid::new_v4());
    let start = std::time::Instant::now();

    // Attempt model discovery (implicitly tests connection)
    let (models, model_source, error) = if protocol == "ollama" {
        let ollama_adapter = super::models::OllamaDiscoveryAdapter::new(
            probe_name.clone(),
            probe_config.clone(),
        );
        match registry
            .list_models(&probe_name, &protocol, &ollama_adapter, &probe_config)
            .await
        {
            models if !models.is_empty() => {
                let source = registry
                    .get_source(&probe_name)
                    .await
                    .map(|s| match s {
                        crate::providers::model_registry::ModelSource::Api => "api".to_string(),
                        crate::providers::model_registry::ModelSource::Preset => "preset".to_string(),
                    })
                    .unwrap_or_else(|| "preset".to_string());
                (models, source, None)
            }
            _ => (vec![], "preset".to_string(), Some("No models found".to_string())),
        }
    } else {
        let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
        if protocol_registry.list_protocols().is_empty() {
            protocol_registry.register_builtin();
        }

        match protocol_registry.get(&protocol) {
            Some(adapter) => {
                let models = registry
                    .list_models(&probe_name, &protocol, adapter.as_ref(), &probe_config)
                    .await;
                if models.is_empty() {
                    (
                        vec![],
                        "preset".to_string(),
                        Some("No models discovered — check API key and endpoint".to_string()),
                    )
                } else {
                    let source = registry
                        .get_source(&probe_name)
                        .await
                        .map(|s| match s {
                            crate::providers::model_registry::ModelSource::Api => "api".to_string(),
                            crate::providers::model_registry::ModelSource::Preset => "preset".to_string(),
                        })
                        .unwrap_or_else(|| "preset".to_string());
                    (models, source, None)
                }
            }
            None => (
                vec![],
                "preset".to_string(),
                Some(format!("Unknown protocol: {}", protocol)),
            ),
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    let success = error.is_none();

    JsonRpcResponse::success(
        request.id,
        json!(ProbeResult {
            success,
            latency_ms: Some(latency_ms),
            models,
            model_source,
            error,
        }),
    )
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
        if let Err(e) = save_config(&cfg) {
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

    #[tokio::test]
    async fn test_needs_setup_empty_providers() {
        let config = Arc::new(RwLock::new(Config::default()));
        let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
        let response = handle_needs_setup(request, config).await;
        let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result["needs_setup"], true);
        assert_eq!(result["provider_count"], 0);
        assert_eq!(result["has_verified"], false);
    }

    #[tokio::test]
    async fn test_needs_setup_has_verified_provider() {
        let mut config = Config::default();
        let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
        provider_cfg.enabled = true;
        provider_cfg.verified = true;
        config.providers.insert("openai".to_string(), provider_cfg);
        let config = Arc::new(RwLock::new(config));
        let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
        let response = handle_needs_setup(request, config).await;
        let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result["needs_setup"], false);
        assert_eq!(result["provider_count"], 1);
        assert_eq!(result["has_verified"], true);
    }

    #[tokio::test]
    async fn test_probe_needs_protocol() {
        let config = Arc::new(RwLock::new(Config::default()));
        let store = Arc::new(
            crate::gateway::security::store::SecurityStore::in_memory()
                .expect("in-memory security store"),
        );
        let vault = Arc::new(SharedTokenManager::new(store, "/tmp/aleph_probe_test.vault"));
        let request = JsonRpcRequest::with_id(
            "providers.probe",
            Some(json!({})),
            serde_json::json!(1),
        );
        let response = handle_probe(request, config, vault).await;
        assert!(response.error.is_some(), "Should fail without protocol");
    }

    #[tokio::test]
    async fn test_probe_unknown_protocol() {
        let config = Arc::new(RwLock::new(Config::default()));
        let store = Arc::new(
            crate::gateway::security::store::SecurityStore::in_memory()
                .expect("in-memory security store"),
        );
        let vault = Arc::new(SharedTokenManager::new(store, "/tmp/aleph_probe_test2.vault"));
        let request = JsonRpcRequest::with_id(
            "providers.probe",
            Some(json!({"protocol": "nonexistent"})),
            serde_json::json!(1),
        );
        let response = handle_probe(request, config, vault).await;
        let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result["success"], false);
    }

    #[tokio::test]
    async fn test_needs_setup_has_unverified_provider() {
        let mut config = Config::default();
        let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
        provider_cfg.enabled = true;
        provider_cfg.verified = false;
        config.providers.insert("openai".to_string(), provider_cfg);
        let config = Arc::new(RwLock::new(config));
        let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
        let response = handle_needs_setup(request, config).await;
        let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(result["needs_setup"], true);
        assert_eq!(result["provider_count"], 1);
        assert_eq!(result["has_verified"], false);
    }
}
