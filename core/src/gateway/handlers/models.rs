//! Models RPC Handlers
//!
//! Handlers for model discovery and capability inspection:
//! - models.list: List available models with filtering
//! - models.get: Get detailed information about a specific model
//! - models.capabilities: Get capability map for a model

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::config::Config;

// ============================================================================
// Types
// ============================================================================

/// Model capabilities
#[derive(Debug, Clone, Serialize, Default)]
pub struct ModelCapabilities {
    /// Supports text input
    pub text: bool,
    /// Supports image/vision input
    pub vision: bool,
    /// Supports tool/function calling
    pub tools: bool,
    /// Supports streaming responses
    pub streaming: bool,
    /// Supports extended thinking/reasoning
    pub thinking: bool,
    /// Supports JSON mode output
    pub json_mode: bool,
}

/// Model information for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "gpt-4o", "claude-3-5-sonnet")
    pub id: String,
    /// Provider name from config (e.g., "openai", "claude")
    pub provider: String,
    /// Provider type (e.g., "openai", "claude", "gemini", "ollama")
    pub provider_type: String,
    /// Whether the provider is enabled
    pub enabled: bool,
    /// Whether this is the default model
    pub is_default: bool,
    /// Model capabilities
    pub capabilities: ModelCapabilities,
}

/// Parameters for models.list
#[derive(Debug, Deserialize, Default)]
pub struct ListParams {
    /// Filter by provider name
    #[serde(default)]
    pub provider: Option<String>,
    /// Only return enabled models
    #[serde(default)]
    pub enabled_only: bool,
}

/// Parameters for models.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    /// Provider name to get model details for
    pub provider: String,
}

// ============================================================================
// Capability Inference
// ============================================================================

/// Infer model capabilities based on provider type and model name
///
/// This function uses heuristics to determine what capabilities a model
/// likely supports based on the provider and model identifier.
pub fn infer_capabilities(provider_type: &str, model: &str) -> ModelCapabilities {
    let model_lower = model.to_lowercase();

    match provider_type {
        "openai" => {
            // GPT-4 vision models
            let vision = model_lower.contains("gpt-4")
                && (model_lower.contains("vision")
                    || model_lower.contains("turbo")
                    || model_lower.contains("o")  // gpt-4o has vision
                    || !model_lower.contains("0314")
                    && !model_lower.contains("0613"));

            // o1/o3 reasoning models have thinking
            let thinking = model_lower.starts_with("o1") || model_lower.starts_with("o3");

            ModelCapabilities {
                text: true,
                vision,
                tools: !thinking, // o1/o3 don't support tools yet
                streaming: !thinking, // o1/o3 don't support streaming
                thinking,
                json_mode: model_lower.contains("gpt-4") || model_lower.contains("gpt-3.5-turbo"),
            }
        }
        "claude" => {
            // Claude 3+ models have vision
            let vision = model_lower.contains("claude-3");

            // Claude 3.5+ sonnet and opus have extended thinking support
            let thinking = model_lower.contains("claude-3-5")
                || model_lower.contains("claude-3.5")
                || model_lower.contains("opus");

            ModelCapabilities {
                text: true,
                vision,
                tools: true,
                streaming: true,
                thinking,
                json_mode: true,
            }
        }
        "gemini" => {
            // Gemini Pro Vision and newer models have vision
            let vision = model_lower.contains("vision")
                || model_lower.contains("pro")
                || model_lower.contains("ultra")
                || model_lower.contains("flash");

            // Gemini 2.0 Flash has thinking support
            let thinking = model_lower.contains("2.0") || model_lower.contains("flash-thinking");

            ModelCapabilities {
                text: true,
                vision,
                tools: true,
                streaming: true,
                thinking,
                json_mode: true,
            }
        }
        "ollama" => {
            // LLaVA and similar models have vision
            let vision = model_lower.contains("llava")
                || model_lower.contains("bakllava")
                || model_lower.contains("vision");

            // Most Ollama models support tools if they're larger models
            let tools = model_lower.contains("llama3")
                || model_lower.contains("mistral")
                || model_lower.contains("mixtral")
                || model_lower.contains("qwen");

            ModelCapabilities {
                text: true,
                vision,
                tools,
                streaming: true,
                thinking: false, // Local models typically don't have extended thinking
                json_mode: tools, // JSON mode usually available when tools are
            }
        }
        _ => {
            // Unknown provider - assume basic capabilities
            ModelCapabilities {
                text: true,
                vision: false,
                tools: false,
                streaming: true,
                thinking: false,
                json_mode: false,
            }
        }
    }
}

// ============================================================================
// Handlers
// ============================================================================

/// List all available models
///
/// # RPC Method
/// `models.list`
///
/// # Parameters
/// - `provider` (optional): Filter by provider name
/// - `enabled_only` (optional, default false): Only return enabled models
///
/// # Returns
/// ```json
/// {
///   "models": [
///     {
///       "id": "gpt-4o",
///       "provider": "openai",
///       "provider_type": "openai",
///       "enabled": true,
///       "is_default": true,
///       "capabilities": { "text": true, "vision": true, ... }
///     }
///   ]
/// }
/// ```
pub async fn handle_list(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: ListParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => ListParams::default(),
    };

    let default_provider = config.general.default_provider.clone();

    let models: Vec<ModelInfo> = config
        .providers
        .iter()
        .filter(|(name, cfg)| {
            // Filter by provider name if specified
            if let Some(ref filter_provider) = params.provider {
                if name.as_str() != filter_provider {
                    return false;
                }
            }
            // Filter by enabled status if requested
            if params.enabled_only && !cfg.enabled {
                return false;
            }
            true
        })
        .map(|(name, cfg)| {
            let provider_type = cfg.infer_provider_type(name);
            let capabilities = infer_capabilities(&provider_type, &cfg.model);

            ModelInfo {
                id: cfg.model.clone(),
                provider: name.clone(),
                provider_type,
                enabled: cfg.enabled,
                is_default: default_provider.as_ref() == Some(name),
                capabilities,
            }
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "models": models }))
}

/// Get detailed information about a specific model
///
/// # RPC Method
/// `models.get`
///
/// # Parameters
/// - `provider` (required): Provider name to get model details for
///
/// # Returns
/// ```json
/// {
///   "model": {
///     "id": "gpt-4o",
///     "provider": "openai",
///     "provider_type": "openai",
///     "enabled": true,
///     "is_default": true,
///     "capabilities": { ... }
///   }
/// }
/// ```
pub async fn handle_get(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
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
                "Missing params: provider required".to_string(),
            );
        }
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let provider_type = cfg.infer_provider_type(&params.provider);
            let capabilities = infer_capabilities(&provider_type, &cfg.model);

            let info = ModelInfo {
                id: cfg.model.clone(),
                provider: params.provider.clone(),
                provider_type,
                enabled: cfg.enabled,
                is_default: default_provider.as_ref() == Some(&params.provider),
                capabilities,
            };

            JsonRpcResponse::success(request.id, json!({ "model": info }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Model not found for provider: {}", params.provider),
        ),
    }
}

/// Get capability map for a specific model
///
/// # RPC Method
/// `models.capabilities`
///
/// # Parameters
/// - `provider` (required): Provider name to get capabilities for
///
/// # Returns
/// ```json
/// {
///   "capabilities": {
///     "text": true,
///     "vision": true,
///     "tools": true,
///     "streaming": true,
///     "thinking": false,
///     "json_mode": true
///   }
/// }
/// ```
pub async fn handle_capabilities(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
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
                "Missing params: provider required".to_string(),
            );
        }
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let provider_type = cfg.infer_provider_type(&params.provider);
            let capabilities = infer_capabilities(&provider_type, &cfg.model);

            JsonRpcResponse::success(request.id, json!({ "capabilities": capabilities }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Model not found for provider: {}", params.provider),
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_params_default() {
        let params: ListParams = serde_json::from_value(json!({})).unwrap();
        assert!(params.provider.is_none());
        assert!(!params.enabled_only);
    }

    #[test]
    fn test_list_params_with_filter() {
        let params: ListParams = serde_json::from_value(json!({
            "provider": "openai",
            "enabled_only": true
        }))
        .unwrap();
        assert_eq!(params.provider, Some("openai".to_string()));
        assert!(params.enabled_only);
    }

    #[test]
    fn test_get_params() {
        let params: GetParams = serde_json::from_value(json!({
            "provider": "claude"
        }))
        .unwrap();
        assert_eq!(params.provider, "claude");
    }

    #[test]
    fn test_model_info_serialize() {
        let info = ModelInfo {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            provider_type: "openai".to_string(),
            enabled: true,
            is_default: true,
            capabilities: ModelCapabilities {
                text: true,
                vision: true,
                tools: true,
                streaming: true,
                thinking: false,
                json_mode: true,
            },
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["id"], "gpt-4o");
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["provider_type"], "openai");
        assert!(json["enabled"].as_bool().unwrap());
        assert!(json["is_default"].as_bool().unwrap());
        assert!(json["capabilities"]["text"].as_bool().unwrap());
        assert!(json["capabilities"]["vision"].as_bool().unwrap());
    }

    #[test]
    fn test_model_capabilities_serialize() {
        let caps = ModelCapabilities {
            text: true,
            vision: true,
            tools: true,
            streaming: true,
            thinking: false,
            json_mode: true,
        };

        let json = serde_json::to_value(&caps).unwrap();
        assert!(json["text"].as_bool().unwrap());
        assert!(json["vision"].as_bool().unwrap());
        assert!(json["tools"].as_bool().unwrap());
        assert!(json["streaming"].as_bool().unwrap());
        assert!(!json["thinking"].as_bool().unwrap());
        assert!(json["json_mode"].as_bool().unwrap());
    }

    #[test]
    fn test_infer_capabilities_openai() {
        // GPT-4o has vision, tools, streaming, json_mode
        let caps = infer_capabilities("openai", "gpt-4o");
        assert!(caps.text);
        assert!(caps.vision);
        assert!(caps.tools);
        assert!(caps.streaming);
        assert!(!caps.thinking);
        assert!(caps.json_mode);

        // GPT-4 turbo has vision
        let caps = infer_capabilities("openai", "gpt-4-turbo");
        assert!(caps.vision);

        // o1 has thinking but no tools/streaming
        let caps = infer_capabilities("openai", "o1-preview");
        assert!(caps.thinking);
        assert!(!caps.tools);
        assert!(!caps.streaming);

        // o3 also has thinking
        let caps = infer_capabilities("openai", "o3-mini");
        assert!(caps.thinking);
    }

    #[test]
    fn test_infer_capabilities_claude() {
        // Claude 3.5 Sonnet has all capabilities
        let caps = infer_capabilities("claude", "claude-3-5-sonnet-20241022");
        assert!(caps.text);
        assert!(caps.vision);
        assert!(caps.tools);
        assert!(caps.streaming);
        assert!(caps.thinking);
        assert!(caps.json_mode);

        // Claude 3 Opus has thinking
        let caps = infer_capabilities("claude", "claude-3-opus");
        assert!(caps.thinking);
        assert!(caps.vision);
    }

    #[test]
    fn test_infer_capabilities_gemini() {
        // Gemini Pro has vision
        let caps = infer_capabilities("gemini", "gemini-pro");
        assert!(caps.text);
        assert!(caps.vision);
        assert!(caps.tools);
        assert!(caps.streaming);
        assert!(caps.json_mode);

        // Gemini 2.0 Flash has thinking
        let caps = infer_capabilities("gemini", "gemini-2.0-flash");
        assert!(caps.thinking);
    }

    #[test]
    fn test_infer_capabilities_ollama() {
        // LLaVA has vision
        let caps = infer_capabilities("ollama", "llava:latest");
        assert!(caps.text);
        assert!(caps.vision);
        assert!(caps.streaming);

        // Llama3 has tools
        let caps = infer_capabilities("ollama", "llama3.2:latest");
        assert!(caps.tools);
        assert!(caps.json_mode);

        // Basic model
        let caps = infer_capabilities("ollama", "phi:latest");
        assert!(caps.text);
        assert!(caps.streaming);
        assert!(!caps.tools);
        assert!(!caps.vision);
    }

    #[test]
    fn test_infer_capabilities_unknown() {
        // Unknown provider gets basic capabilities
        let caps = infer_capabilities("unknown", "some-model");
        assert!(caps.text);
        assert!(caps.streaming);
        assert!(!caps.vision);
        assert!(!caps.tools);
        assert!(!caps.thinking);
        assert!(!caps.json_mode);
    }

    #[tokio::test]
    async fn test_handle_list_empty_config() {
        let config = Arc::new(Config::default());
        let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

        let response = handle_list(request, config).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let models = result["models"].as_array().unwrap();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn test_handle_list_with_providers() {
        use crate::config::ProviderConfig;

        let mut config = Config::default();
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig::test_config("gpt-4o"),
        );
        config.providers.insert(
            "claude".to_string(),
            ProviderConfig::test_config("claude-3-5-sonnet-20241022"),
        );
        config.general.default_provider = Some("openai".to_string());

        let config = Arc::new(config);
        let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

        let response = handle_list(request, config).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
    }

    #[tokio::test]
    async fn test_handle_list_with_filter() {
        use crate::config::ProviderConfig;

        let mut config = Config::default();
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig::test_config("gpt-4o"),
        );
        config.providers.insert(
            "claude".to_string(),
            ProviderConfig::test_config("claude-3-5-sonnet-20241022"),
        );

        let config = Arc::new(config);
        let request = JsonRpcRequest::new(
            "models.list",
            Some(json!({ "provider": "openai" })),
            Some(json!(1)),
        );

        let response = handle_list(request, config).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["provider"], "openai");
    }

    #[tokio::test]
    async fn test_handle_get_success() {
        use crate::config::ProviderConfig;

        let mut config = Config::default();
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig::test_config("gpt-4o"),
        );

        let config = Arc::new(config);
        let request = JsonRpcRequest::new(
            "models.get",
            Some(json!({ "provider": "openai" })),
            Some(json!(1)),
        );

        let response = handle_get(request, config).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["model"]["id"], "gpt-4o");
        assert_eq!(result["model"]["provider"], "openai");
    }

    #[tokio::test]
    async fn test_handle_get_not_found() {
        let config = Arc::new(Config::default());
        let request = JsonRpcRequest::new(
            "models.get",
            Some(json!({ "provider": "nonexistent" })),
            Some(json!(1)),
        );

        let response = handle_get(request, config).await;

        assert!(response.is_error());
        assert!(response.error.unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn test_handle_get_missing_params() {
        let config = Arc::new(Config::default());
        let request = JsonRpcRequest::new("models.get", None, Some(json!(1)));

        let response = handle_get(request, config).await;

        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_capabilities_success() {
        use crate::config::ProviderConfig;

        let mut config = Config::default();
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig::test_config("gpt-4o"),
        );

        let config = Arc::new(config);
        let request = JsonRpcRequest::new(
            "models.capabilities",
            Some(json!({ "provider": "openai" })),
            Some(json!(1)),
        );

        let response = handle_capabilities(request, config).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["capabilities"]["text"].as_bool().unwrap());
        assert!(result["capabilities"]["vision"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_capabilities_not_found() {
        let config = Arc::new(Config::default());
        let request = JsonRpcRequest::new(
            "models.capabilities",
            Some(json!({ "provider": "nonexistent" })),
            Some(json!(1)),
        );

        let response = handle_capabilities(request, config).await;

        assert!(response.is_error());
    }
}
