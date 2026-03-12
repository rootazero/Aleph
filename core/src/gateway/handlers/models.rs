//! Models RPC Handlers
//!
//! Handlers for model discovery and capability inspection:
//! - models.list: List available models with filtering
//! - models.get: Get detailed information about a specific model
//! - models.capabilities: Get capability map for a model
//! - models.refresh: Force refresh model lists from providers
//! - models.set_default: Set the default provider
//! - models.set_model: Change a provider's configured model

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::parse_params;
use crate::config::Config;

// ============================================================================
// OllamaDiscoveryAdapter
// ============================================================================

/// Adapter that wraps OllamaProvider::list_models() for ModelRegistry caching
struct OllamaDiscoveryAdapter {
    name: String,
    config: crate::config::ProviderConfig,
}

impl OllamaDiscoveryAdapter {
    fn new(name: String, config: crate::config::ProviderConfig) -> Self {
        Self { name, config }
    }
}

#[async_trait::async_trait]
impl crate::providers::ProtocolAdapter for OllamaDiscoveryAdapter {
    fn build_request(
        &self,
        _payload: &crate::providers::RequestPayload,
        _config: &crate::config::ProviderConfig,
        _is_streaming: bool,
    ) -> crate::error::Result<reqwest::RequestBuilder> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    async fn parse_response(
        &self,
        _response: reqwest::Response,
    ) -> crate::error::Result<crate::providers::ProviderResponse> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    async fn parse_stream(
        &self,
        _response: reqwest::Response,
    ) -> crate::error::Result<futures::stream::BoxStream<'static, crate::error::Result<String>>> {
        unimplemented!("OllamaDiscoveryAdapter is only used for list_models")
    }
    fn name(&self) -> &'static str {
        "ollama-discovery"
    }
    async fn list_models(
        &self,
        _config: &crate::config::ProviderConfig,
    ) -> crate::error::Result<Option<Vec<crate::providers::adapter::DiscoveredModel>>> {
        match crate::providers::OllamaProvider::new(self.name.clone(), self.config.clone()) {
            Ok(provider) => Ok(Some(provider.list_models().await.unwrap_or_default())),
            Err(_) => Ok(None),
        }
    }
}

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
    /// Whether this is the provider's currently configured model
    pub is_current: bool,
    /// Model capabilities as string list: "chat", "vision", "tools", "thinking"
    pub capabilities: Vec<String>,
    /// Source of this model info: "api", "preset", "config"
    pub source: String,
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
    /// Force cache refresh before listing
    #[serde(default)]
    pub refresh: bool,
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
/// - `refresh` (optional, default false): Force cache refresh before listing
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
///       "is_current": true,
///       "capabilities": ["chat", "vision", "tools"],
///       "source": "api"
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
    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let mut all_models = Vec::new();

    for (name, cfg) in &config.providers {
        if let Some(ref filter) = params.provider {
            if name != filter {
                continue;
            }
        }
        if params.enabled_only && !cfg.enabled {
            continue;
        }

        let protocol = cfg.protocol();
        let is_default = default_provider.as_ref() == Some(name);

        let discovered = if protocol == "ollama" {
            let ollama_adapter = OllamaDiscoveryAdapter::new(name.clone(), cfg.clone());
            if params.refresh {
                registry.refresh(name, &protocol, &ollama_adapter, cfg).await
            } else {
                registry.list_models(name, &protocol, &ollama_adapter, cfg).await
            }
        } else {
            match protocol_registry.get(&protocol) {
                Some(adapter) => {
                    if params.refresh {
                        registry.refresh(name, &protocol, adapter.as_ref(), cfg).await
                    } else {
                        registry.list_models(name, &protocol, adapter.as_ref(), cfg).await
                    }
                }
                None => vec![],
            }
        };

        if discovered.is_empty() {
            all_models.push(ModelInfo {
                id: cfg.model.clone(),
                provider: name.clone(),
                provider_type: protocol.clone(),
                enabled: cfg.enabled,
                is_default,
                is_current: true,
                capabilities: infer_capabilities(&protocol, &cfg.model),
                source: "config".to_string(),
            });
        } else {
            let source = registry
                .get_source(name)
                .await
                .map(|s| match s {
                    crate::providers::model_registry::ModelSource::Api => "api",
                    crate::providers::model_registry::ModelSource::Preset => "preset",
                })
                .unwrap_or("config");

            for model in discovered {
                let is_current = model.id == cfg.model;
                let capabilities = if model.capabilities.is_empty() {
                    infer_capabilities(&protocol, &model.id)
                } else {
                    model.capabilities.clone()
                };

                all_models.push(ModelInfo {
                    id: model.id,
                    provider: name.clone(),
                    provider_type: protocol.clone(),
                    enabled: cfg.enabled,
                    is_default: is_default && is_current,
                    is_current,
                    capabilities,
                    source: source.to_string(),
                });
            }
        }
    }

    JsonRpcResponse::success(request.id, json!({ "models": all_models }))
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
///     "is_current": true,
///     "capabilities": ["chat", "vision", "tools"],
///     "source": "config"
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
                is_current: true,
                capabilities,
                source: "config".to_string(),
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

/// Parameters for models.refresh
#[derive(Debug, Deserialize, Default)]
pub struct RefreshParams {
    #[serde(default)]
    pub provider: Option<String>,
}

/// Force refresh model list for a provider
pub async fn handle_refresh(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: RefreshParams = match &request.params {
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
        None => RefreshParams::default(),
    };

    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let providers_to_refresh: Vec<_> = config
        .providers
        .iter()
        .filter(|(name, _)| {
            params.provider.as_ref().map_or(true, |filter| name.as_str() == filter)
        })
        .collect();

    let mut results = Vec::new();

    for (name, cfg) in providers_to_refresh {
        let protocol = cfg.protocol();

        let models = if protocol == "ollama" {
            let ollama_adapter = OllamaDiscoveryAdapter::new(name.clone(), cfg.clone());
            registry.refresh(name, &protocol, &ollama_adapter, cfg).await
        } else {
            match protocol_registry.get(&protocol) {
                Some(adapter) => {
                    registry.refresh(name, &protocol, adapter.as_ref(), cfg).await
                }
                None => vec![],
            }
        };

        let source = registry
            .get_source(name)
            .await
            .map(|s| match s {
                crate::providers::model_registry::ModelSource::Api => "api",
                crate::providers::model_registry::ModelSource::Preset => "preset",
            })
            .unwrap_or("config");

        results.push(json!({
            "provider": name,
            "count": models.len(),
            "source": source,
            "models": models.iter().map(|m| json!({
                "id": m.id,
                "name": m.name,
                "capabilities": m.capabilities,
            })).collect::<Vec<_>>(),
        }));
    }

    if results.len() == 1 {
        JsonRpcResponse::success(request.id, results.into_iter().next().unwrap())
    } else {
        JsonRpcResponse::success(request.id, json!({ "results": results }))
    }
}

/// Parameters for models.set_default
#[derive(Debug, Deserialize)]
pub struct SetDefaultParams {
    pub provider: String,
}

/// Set the default provider
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut cfg = config.write().await;

    if !cfg.providers.contains_key(&params.provider) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.provider),
        );
    }

    cfg.general.default_provider = Some(params.provider.clone());

    JsonRpcResponse::success(
        request.id,
        json!({ "message": format!("Default provider set to {}", params.provider) }),
    )
}

/// Parameters for models.set_model
#[derive(Debug, Deserialize)]
pub struct SetModelParams {
    pub provider: String,
    pub model: String,
}

/// Change a provider's configured model with validation
pub async fn handle_set_model(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    let params: SetModelParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut cfg = config.write().await;

    let provider_cfg = match cfg.providers.get(&params.provider) {
        Some(c) => c.clone(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.provider),
            );
        }
    };

    // Validate model exists in available models
    let registry = &crate::providers::model_registry::MODEL_REGISTRY;
    let protocol = provider_cfg.protocol();
    let protocol_registry = crate::providers::protocols::ProtocolRegistry::global();
    if protocol_registry.list_protocols().is_empty() {
        protocol_registry.register_builtin();
    }

    let available = if protocol == "ollama" {
        let ollama_adapter = OllamaDiscoveryAdapter::new(params.provider.clone(), provider_cfg.clone());
        registry.list_models(&params.provider, &protocol, &ollama_adapter, &provider_cfg).await
    } else {
        match protocol_registry.get(&protocol) {
            Some(adapter) => {
                registry
                    .list_models(&params.provider, &protocol, adapter.as_ref(), &provider_cfg)
                    .await
            }
            None => vec![],
        }
    };

    // If we have a model list, validate against it
    if !available.is_empty() && !available.iter().any(|m| m.id == params.model) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Model '{}' not found in {}'s available models. Available: {}",
                params.model,
                params.provider,
                available.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ")
            ),
        );
    }

    // Update the model
    if let Some(provider) = cfg.providers.get_mut(&params.provider) {
        provider.model = params.model.clone();
    }

    JsonRpcResponse::success(
        request.id,
        json!({ "message": format!("Provider {} model set to {}", params.provider, params.model) }),
    )
}

/// Set the active model (default provider) — backward compatibility alias
///
/// DEPRECATED: Use models.set_default instead
pub async fn handle_set(
    request: JsonRpcRequest,
    config: Arc<tokio::sync::RwLock<Config>>,
) -> JsonRpcResponse {
    // Translate old params format { "model": "openai" } to new { "provider": "openai" }
    let new_params = request.params.as_ref().and_then(|p| {
        p.get("model").map(|m| json!({ "provider": m }))
    });

    let new_request = JsonRpcRequest::new(
        "models.set_default",
        new_params,
        request.id.clone(),
    );

    handle_set_default(new_request, config).await
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
        assert!(!params.refresh);
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
    fn test_list_params_with_refresh() {
        let params: ListParams = serde_json::from_value(json!({
            "refresh": true
        }))
        .unwrap();
        assert!(params.refresh);
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
            is_current: true,
            capabilities: vec![
                "chat".to_string(),
                "vision".to_string(),
                "tools".to_string(),
            ],
            source: "config".to_string(),
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["id"], "gpt-4o");
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["provider_type"], "openai");
        assert!(json["enabled"].as_bool().unwrap());
        assert!(json["is_default"].as_bool().unwrap());
        assert!(json["is_current"].as_bool().unwrap());
        assert_eq!(json["source"], "config");
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
        // With registry, each provider returns preset models (not just 1 config model)
        assert!(!models.is_empty());
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
        // All returned models should be from openai provider
        for m in models {
            assert_eq!(m["provider"], "openai");
        }
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
        assert!(result["model"]["is_current"].as_bool().unwrap());
        assert_eq!(result["model"]["source"], "config");
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
