//! Models RPC handler implementations.

use serde_json::json;
use crate::sync_primitives::Arc;

use super::super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::super::parse_params;
use crate::config::Config;

use super::types::{
    GetParams, ListParams, ModelInfo, RefreshParams, SetDefaultParams, SetModelParams,
    infer_capabilities,
};
use super::OllamaDiscoveryAdapter;

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
                id: cfg.default_model().to_string(),
                provider: name.clone(),
                provider_type: protocol.clone(),
                enabled: cfg.enabled,
                is_default,
                is_current: true,
                capabilities: infer_capabilities(&protocol, cfg.default_model()),
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
                let is_current = model.id == cfg.default_model();
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
            let capabilities = infer_capabilities(&protocol, cfg.default_model());

            let info = ModelInfo {
                id: cfg.default_model().to_string(),
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
            let capabilities = infer_capabilities(&protocol, cfg.default_model());

            JsonRpcResponse::success(request.id, json!({ "capabilities": capabilities }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Model not found for provider: {}", params.provider),
        ),
    }
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

    // Validate specific provider filter exists in config
    if let Some(ref filter) = params.provider {
        if !config.providers.contains_key(filter.as_str()) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", filter),
            );
        }
    }

    let providers_to_refresh: Vec<_> = config
        .providers
        .iter()
        .filter(|(name, _)| {
            params.provider.as_ref().is_none_or(|filter| name.as_str() == filter)
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
        provider.models = vec![params.model.clone()];
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
