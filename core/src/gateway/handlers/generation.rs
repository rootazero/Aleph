//! Generation Providers RPC Handlers
//!
//! Handlers for generation provider management (image, audio, etc.).

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Generation provider info
#[derive(Debug, Clone, Serialize)]
pub struct GenerationProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub provider_type: String, // "image", "audio", "tts"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Test result
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// List Providers
// ============================================================================

/// List all generation providers
pub async fn handle_list_providers(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from config
    // For now, return empty list
    JsonRpcResponse::success(
        request.id,
        json!({ "providers": [] as Vec<GenerationProviderInfo> }),
    )
}

// ============================================================================
// Get Provider
// ============================================================================

/// Parameters for generation.getProvider
#[derive(Debug, Deserialize)]
pub struct GetProviderParams {
    pub name: String,
}

/// Get a generation provider config
pub async fn handle_get_provider(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: GetProviderParams = match request.params {
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

    // TODO: Get from config
    JsonRpcResponse::error(
        request.id,
        INVALID_PARAMS,
        format!("Provider not found: {}", params.name),
    )
}

// ============================================================================
// Update Provider
// ============================================================================

/// Parameters for generation.updateProvider
#[derive(Debug, Deserialize)]
pub struct UpdateProviderParams {
    pub name: String,
    pub config: GenerationProviderConfigJson,
}

/// Generation provider config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct GenerationProviderConfigJson {
    #[serde(default)]
    pub enabled: bool,
    pub provider_type: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Update a generation provider
pub async fn handle_update_provider(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UpdateProviderParams = match request.params {
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

    // TODO: Update config
    tracing::info!(name = %params.name, "Generation provider updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Test Provider
// ============================================================================

/// Parameters for generation.testProvider
#[derive(Debug, Deserialize)]
pub struct TestProviderParams {
    pub name: String,
    pub config: GenerationProviderConfigJson,
}

/// Test a generation provider
pub async fn handle_test_provider(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: TestProviderParams = match request.params {
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

    // TODO: Actually test the provider
    JsonRpcResponse::success(
        request.id,
        json!(TestResult {
            success: true,
            error: None,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_params() {
        let json = json!({
            "name": "dalle",
            "config": {
                "enabled": true,
                "provider_type": "image",
                "model": "dall-e-3"
            }
        });
        let params: UpdateProviderParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "dalle");
        assert_eq!(params.config.provider_type, "image");
    }
}
