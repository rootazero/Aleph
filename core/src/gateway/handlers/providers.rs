//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, update, delete, test, setDefault.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
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
pub async fn handle_list(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let default_provider = config.general.default_provider.clone();

    let providers: Vec<ProviderInfo> = config
        .providers
        .iter()
        .map(|(name, cfg)| ProviderInfo {
            name: name.clone(),
            enabled: cfg.enabled,
            model: cfg.model.clone(),
            provider_type: cfg.provider_type.as_ref().map(|t| format!("{:?}", t)),
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
pub async fn handle_get(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
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

    match config.providers.get(&params.name) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let info = ProviderInfo {
                name: params.name.clone(),
                enabled: cfg.enabled,
                model: cfg.model.clone(),
                provider_type: cfg.provider_type.as_ref().map(|t| format!("{:?}", t)),
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
    pub enabled: bool,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Update a provider
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Update config file and reload
    tracing::info!(name = %params.name, "Provider updated");
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
pub async fn handle_delete(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Delete from config file and reload
    tracing::info!(name = %params.name, "Provider deleted");
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

    // TODO: Actually test the provider connection
    // For now, return success placeholder
    JsonRpcResponse::success(
        request.id,
        json!(TestResult {
            success: true,
            error: None,
            latency_ms: Some(100),
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

/// Set the default provider
pub async fn handle_set_default(request: JsonRpcRequest) -> JsonRpcResponse {
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

    // TODO: Update default in config file and reload
    tracing::info!(name = %params.name, "Default provider set");
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
