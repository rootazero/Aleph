//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, create, update, delete, test, setDefault.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, ProviderConfig};

/// Provider info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    pub is_default: bool,
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
            is_default: default_provider.as_ref() == Some(name),
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
    let params: GetParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name required".to_string(),
            );
        }
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
                is_default: default_provider.as_ref() == Some(&params.name),
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

/// Update a provider
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name, config required".to_string(),
            );
        }
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
        let provider_config = ProviderConfig {
            protocol: params.config.protocol,
            api_key: params.config.api_key,
            model: params.config.model.clone(),
            base_url: params.config.base_url,
            color: params.config.color.unwrap_or_else(|| "#808080".to_string()),
            timeout_seconds: params.config.timeout_seconds.unwrap_or(300),
            enabled: params.config.enabled,
            max_tokens: params.config.max_tokens,
            temperature: params.config.temperature,
            top_p: params.config.top_p,
            top_k: params.config.top_k,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
        };

        // Update provider
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = cfg.save() {
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
    let params: CreateParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name, config required".to_string(),
            );
        }
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
        let provider_config = ProviderConfig {
            protocol: params.config.protocol,
            api_key: params.config.api_key,
            model: params.config.model.clone(),
            base_url: params.config.base_url,
            color: params.config.color.unwrap_or_else(|| "#808080".to_string()),
            timeout_seconds: params.config.timeout_seconds.unwrap_or(300),
            enabled: params.config.enabled,
            max_tokens: params.config.max_tokens,
            temperature: params.config.temperature,
            top_p: params.config.top_p,
            top_k: params.config.top_k,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
        };

        // Insert provider
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = cfg.save() {
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
    let params: DeleteParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name required".to_string(),
            );
        }
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

        // Save to file
        if let Err(e) = cfg.save() {
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
    pub config: ProviderConfigJson,
}

/// Test a provider connection
pub async fn handle_test(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: TestParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: config required".to_string(),
            );
        }
    };

    // Convert JSON config to ProviderConfig
    let provider_config = ProviderConfig {
        protocol: params.config.protocol,
        api_key: params.config.api_key,
        model: params.config.model.clone(),
        base_url: params.config.base_url,
        color: params.config.color.unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: params.config.timeout_seconds.unwrap_or(300),
        enabled: params.config.enabled,
        max_tokens: params.config.max_tokens,
        temperature: params.config.temperature,
        top_p: params.config.top_p,
        top_k: params.config.top_k,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
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

/// Set the default provider
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: name required".to_string(),
            );
        }
    };

    // Set default provider
    {
        let mut cfg = config.write().await;

        // Use the existing set_default_provider method
        if let Err(e) = cfg.set_default_provider(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Failed to set default provider: {}", e),
            );
        }

        // Save to file
        if let Err(e) = cfg.save() {
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
        value: json!({ "action": "set_default", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Default provider set");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
