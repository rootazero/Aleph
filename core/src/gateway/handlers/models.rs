//! Models RPC Handlers
//!
//! Handlers for model discovery and capability inspection:
//! - models.list: List available models with filtering
//! - models.get: Get detailed information about a specific model
//! - models.capabilities: Get capability map for a model

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::parse_params;
use crate::config::Config;

// ============================================================================
// Types
// ============================================================================

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
    /// Model capabilities as string list: "chat", "vision", "tools", "thinking"
    pub capabilities: Vec<String>,
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
///
/// Returns a list of capability strings: "chat", "vision", "tools", "thinking"
pub fn infer_capabilities(provider_type: &str, model: &str) -> Vec<String> {
    let model_lower = model.to_lowercase();
    let mut capabilities = vec!["chat".to_string()]; // All models support chat

    match provider_type {
        "openai" => {
            // GPT-4 vision models
            let has_vision = model_lower.contains("gpt-4")
                && (model_lower.contains("vision")
                    || model_lower.contains("turbo")
                    || model_lower.ends_with("o") || model_lower.contains("-o-") // gpt-4o has vision
                    || !model_lower.contains("0314") && !model_lower.contains("0613"));

            // o1/o3 reasoning models have thinking
            let has_thinking =
                model_lower.starts_with("o1") || model_lower.starts_with("o3");

            if has_vision {
                capabilities.push("vision".to_string());
            }
            if !has_thinking {
                // o1/o3 don't support tools yet
                capabilities.push("tools".to_string());
            }
            if has_thinking {
                capabilities.push("thinking".to_string());
            }
        }
        "anthropic" => {
            // Claude 3+ models have vision
            if model_lower.contains("claude-3") {
                capabilities.push("vision".to_string());
            }

            // All Claude models support tools
            capabilities.push("tools".to_string());

            // Claude 3.5+ sonnet and opus have extended thinking support
            if model_lower.contains("claude-3-5")
                || model_lower.contains("claude-3.5")
                || model_lower.contains("opus")
            {
                capabilities.push("thinking".to_string());
            }
        }
        "gemini" => {
            // Gemini Pro Vision and newer models have vision
            if model_lower.contains("vision")
                || model_lower.contains("pro")
                || model_lower.contains("ultra")
                || model_lower.contains("flash")
            {
                capabilities.push("vision".to_string());
            }

            // All Gemini models support tools
            capabilities.push("tools".to_string());

            // Gemini 2.0 Flash has thinking support
            if model_lower.contains("2.0") || model_lower.contains("flash-thinking") {
                capabilities.push("thinking".to_string());
            }
        }
        "ollama" => {
            // LLaVA and similar models have vision
            if model_lower.contains("llava")
                || model_lower.contains("bakllava")
                || model_lower.contains("vision")
            {
                capabilities.push("vision".to_string());
            }

            // Most Ollama models support tools if they're larger models
            if model_lower.contains("llama3")
                || model_lower.contains("mistral")
                || model_lower.contains("mixtral")
                || model_lower.contains("qwen")
            {
                capabilities.push("tools".to_string());
            }

            // Local models typically don't have extended thinking
        }
        _ => {
            // Unknown provider - only basic chat capability
        }
    }

    capabilities
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
///       "capabilities": ["chat", "vision", "tools"]
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
            let protocol = cfg.protocol();
            let capabilities = infer_capabilities(&protocol, &cfg.model);

            ModelInfo {
                id: cfg.model.clone(),
                provider: name.clone(),
                provider_type: protocol,
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
///     "capabilities": ["chat", "vision", "tools"]
///   }
/// }
/// ```
pub async fn handle_get(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let protocol = cfg.protocol();
            let capabilities = infer_capabilities(&protocol, &cfg.model);

            let info = ModelInfo {
                id: cfg.model.clone(),
                provider: params.provider.clone(),
                provider_type: protocol,
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
///   "capabilities": ["chat", "vision", "tools", "thinking"]
/// }
/// ```
pub async fn handle_capabilities(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let protocol = cfg.protocol();
            let capabilities = infer_capabilities(&protocol, &cfg.model);

            JsonRpcResponse::success(request.id, json!({ "capabilities": capabilities }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Model not found for provider: {}", params.provider),
        ),
    }
}

/// Parameters for models.set
#[derive(Debug, Deserialize)]
pub struct SetParams {
    /// Model/provider name to set as active
    pub model: String,
}

/// Set the active model (default provider)
///
/// # RPC Method
/// `models.set`
///
/// # Parameters
/// - `model` (required): Provider name to set as the default
///
/// # Returns
/// ```json
/// {
///   "model": "openai",
///   "message": "Default model set to: openai"
/// }
/// ```
pub async fn handle_set(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    let params: SetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut cfg = config.write().await;

    // Verify the provider exists
    if !cfg.providers.contains_key(&params.model) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.model),
        );
    }

    cfg.general.default_provider = Some(params.model.clone());

    JsonRpcResponse::success(
        request.id,
        json!({
            "model": params.model,
            "message": format!("Default model set to: {}", params.model),
        }),
    )
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
            capabilities: vec![
                "chat".to_string(),
                "vision".to_string(),
                "tools".to_string(),
            ],
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["id"], "gpt-4o");
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["provider_type"], "openai");
        assert!(json["enabled"].as_bool().unwrap());
        assert!(json["is_default"].as_bool().unwrap());
        let caps = json["capabilities"].as_array().unwrap();
        assert!(caps.iter().any(|c| c == "chat"));
        assert!(caps.iter().any(|c| c == "vision"));
        assert!(caps.iter().any(|c| c == "tools"));
    }

    #[test]
    fn test_capabilities_serialize() {
        let caps = vec![
            "chat".to_string(),
            "vision".to_string(),
            "tools".to_string(),
        ];

        let json = serde_json::to_value(&caps).unwrap();
        let arr = json.as_array().unwrap();
        assert!(arr.iter().any(|c| c == "chat"));
        assert!(arr.iter().any(|c| c == "vision"));
        assert!(arr.iter().any(|c| c == "tools"));
    }

    #[test]
    fn test_infer_capabilities_openai() {
        // GPT-4o has chat, vision, tools
        let caps = infer_capabilities("openai", "gpt-4o");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));
        assert!(caps.contains(&"tools".to_string()));
        assert!(!caps.contains(&"thinking".to_string()));

        // GPT-4 turbo has vision
        let caps = infer_capabilities("openai", "gpt-4-turbo");
        assert!(caps.contains(&"vision".to_string()));

        // o1 has thinking but no tools
        let caps = infer_capabilities("openai", "o1-preview");
        assert!(caps.contains(&"thinking".to_string()));
        assert!(!caps.contains(&"tools".to_string()));

        // o3 also has thinking
        let caps = infer_capabilities("openai", "o3-mini");
        assert!(caps.contains(&"thinking".to_string()));
    }

    #[test]
    fn test_infer_capabilities_claude() {
        // Claude 3.5 Sonnet has chat, vision, tools, thinking
        let caps = infer_capabilities("anthropic", "claude-3-5-sonnet-20241022");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));
        assert!(caps.contains(&"tools".to_string()));
        assert!(caps.contains(&"thinking".to_string()));

        // Claude 3 Opus has thinking and vision
        let caps = infer_capabilities("anthropic", "claude-3-opus");
        assert!(caps.contains(&"thinking".to_string()));
        assert!(caps.contains(&"vision".to_string()));
    }

    #[test]
    fn test_infer_capabilities_gemini() {
        // Gemini Pro has chat, vision, tools
        let caps = infer_capabilities("gemini", "gemini-pro");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));
        assert!(caps.contains(&"tools".to_string()));

        // Gemini 2.0 Flash has thinking
        let caps = infer_capabilities("gemini", "gemini-2.0-flash");
        assert!(caps.contains(&"thinking".to_string()));
    }

    #[test]
    fn test_infer_capabilities_ollama() {
        // LLaVA has chat and vision
        let caps = infer_capabilities("ollama", "llava:latest");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));

        // Llama3 has tools
        let caps = infer_capabilities("ollama", "llama3.2:latest");
        assert!(caps.contains(&"tools".to_string()));

        // Basic model only has chat
        let caps = infer_capabilities("ollama", "phi:latest");
        assert!(caps.contains(&"chat".to_string()));
        assert!(!caps.contains(&"tools".to_string()));
        assert!(!caps.contains(&"vision".to_string()));
    }

    #[test]
    fn test_infer_capabilities_unknown() {
        // Unknown provider gets only chat capability
        let caps = infer_capabilities("unknown", "some-model");
        assert!(caps.contains(&"chat".to_string()));
        assert!(!caps.contains(&"vision".to_string()));
        assert!(!caps.contains(&"tools".to_string()));
        assert!(!caps.contains(&"thinking".to_string()));
    }

    #[tokio::test]
    async fn test_handle_list_empty_config() {
        let config = Arc::new(Config::default());
        let request = JsonRpcRequest::with_id("models.list", None, json!(1));

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
        let request = JsonRpcRequest::with_id("models.list", None, json!(1));

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
        let request = JsonRpcRequest::with_id("models.get", None, json!(1));

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
        let caps = result["capabilities"].as_array().unwrap();
        assert!(caps.iter().any(|c| c == "chat"));
        assert!(caps.iter().any(|c| c == "vision"));
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
