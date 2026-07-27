//! Embedding Providers RPC handlers
//!
//! Provides RPC methods for managing embedding providers (vector embedding services).
//!
//! | Method | Description |
//! |--------|-------------|
//! | embedding_providers.list | List all configured embedding providers |
//! | embedding_providers.get | Get a single provider by id |
//! | embedding_providers.add | Add a new provider config |
//! | embedding_providers.update | Update an existing provider |
//! | embedding_providers.remove | Remove a provider by id |
//! | embedding_providers.setActive | Set the active provider |
//! | embedding_providers.test | Test provider connectivity |
//! | embedding_providers.probe | Probe for available embedding models |
//! | embedding_providers.presets | Return preset configurations |

use crate::config::types::memory::{EmbeddingPreset, EmbeddingProviderConfig};
use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::memory::embedding_provider::RemoteEmbeddingProvider;
use crate::sync_primitives::Arc;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{error, warn};

/// Get preset defaults for an embedding provider based on its preset field.
/// Returns (`default_api_base`, `default_model`).
const fn get_embedding_preset_defaults(
    preset: &EmbeddingPreset,
) -> Option<(&'static str, &'static str)> {
    match preset {
        EmbeddingPreset::SiliconFlow => Some(("https://api.siliconflow.cn/v1", "BAAI/bge-m3")),
        EmbeddingPreset::OpenAi => Some(("https://api.openai.com/v1", "text-embedding-3-small")),
        EmbeddingPreset::Ollama => Some(("http://localhost:11434/v1", "nomic-embed-text")),
        EmbeddingPreset::Jina => Some(("https://api.jina.ai/v1", "jina-embeddings-v3")),
        EmbeddingPreset::Mistral => Some(("https://api.mistral.ai/v1", "mistral-embed")),
        EmbeddingPreset::Custom => None,
    }
}

/// Restore preset defaults for empty `api_base` / model fields.
fn apply_embedding_preset_defaults(config: &mut EmbeddingProviderConfig) {
    if let Some((default_base, default_model)) = get_embedding_preset_defaults(&config.preset) {
        if config.api_base.trim().is_empty() {
            config.api_base = default_base.to_string();
        }
        if config.default_model().trim().is_empty() {
            config.models = vec![default_model.to_string()];
        }
    }
}

/// Vault key prefix for embedding provider API keys
fn vault_key(provider_id: &str) -> String {
    format!("embed:{provider_id}")
}

/// Resolve API key from vault for an embedding provider
fn resolve_api_key(id: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(id), vault)
}

fn save_config(cfg: &Config) -> Result<(), String> {
    cfg.save_incremental(&["memory"]).map_err(|e| e.to_string())
}

/// Serialize a provider config to JSON and inject `is_active` based on the active provider id.
/// The `verified` field is already part of `EmbeddingProviderConfig` and serialized automatically.
fn inject_is_active(provider: &EmbeddingProviderConfig, active_id: &str) -> serde_json::Value {
    let mut val = serde_json::to_value(provider).unwrap_or_default();
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "is_active".into(),
            serde_json::json!(provider.id == active_id),
        );
        // Panel expects "model" (string), core serializes "models" (array)
        if !obj.contains_key("model") {
            obj.insert("model".into(), serde_json::json!(provider.default_model()));
        }
    }
    val
}

// =============================================================================
// RPC Handlers
// =============================================================================

/// List all configured embedding providers with `is_active` flag.
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let settings = &cfg.memory.embedding;

    let providers: Vec<serde_json::Value> = settings
        .providers
        .iter()
        .map(|p| {
            let mut val = inject_is_active(p, &settings.active_provider_id);
            if let Some(obj) = val.as_object_mut() {
                let has_api_key = resolve_api_key(&p.id, &vault).is_some();
                obj.insert("has_api_key".into(), serde_json::json!(has_api_key));
            }
            val
        })
        .collect();

    JsonRpcResponse::success(request.id, serde_json::json!(providers))
}

/// Get a single embedding provider by id.
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cfg = config.read().await;
    let settings = &cfg.memory.embedding;

    match settings.providers.iter().find(|p| p.id == params.id) {
        Some(provider) => {
            let mut val = inject_is_active(provider, &settings.active_provider_id);
            if let Some(obj) = val.as_object_mut() {
                let has_api_key = resolve_api_key(&params.id, &vault).is_some();
                obj.insert("has_api_key".into(), serde_json::json!(has_api_key));
            }
            JsonRpcResponse::success(request.id, val)
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Embedding provider '{}' not found", params.id),
        ),
    }
}

/// Add a new embedding provider config (validate id uniqueness).
pub async fn handle_add(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        config: EmbeddingProviderConfig,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut provider_config = params.config;
    apply_embedding_preset_defaults(&mut provider_config);

    // Take the API key but don't persist it until we've confirmed this is a NEW
    // provider — writing to the vault before the duplicate-id check would clobber
    // an existing provider's stored key on a failed "add".
    let api_key = provider_config.api_key.take().filter(|k| !k.is_empty());

    {
        let mut cfg = config.write().await;

        // Check if provider id already exists
        if cfg
            .memory
            .embedding
            .providers
            .iter()
            .any(|p| p.id == provider_config.id)
        {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Embedding provider '{}' already exists", provider_config.id),
            );
        }

        // id confirmed new — now persist the API key to the vault.
        if let Some(ref api_key) = api_key {
            if let Err(e) = vault.store_secret(&vault_key(&provider_config.id), api_key) {
                error!(error = %e, "Failed to store embedding API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {e}"),
                );
            }
        }

        // Add provider
        cfg.memory.embedding.providers.push(provider_config.clone());

        tracing::info!(
            provider_id = %provider_config.id,
            total_providers = cfg.memory.embedding.providers.len(),
            "Embedding provider added to in-memory config, saving to disk..."
        );

        // Save to file
        if let Err(e) = save_config(&cfg) {
            error!(error = %e, provider_id = %provider_config.id, "Failed to save embedding provider config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }

        tracing::info!(
            provider_id = %provider_config.id,
            "Embedding provider saved to config.toml successfully"
        );
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "added",
        "provider_id": provider_config.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Update an existing embedding provider by id.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        config: EmbeddingProviderConfig,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Find and update the provider — config change resets verified
        let provider = cfg
            .memory
            .embedding
            .providers
            .iter_mut()
            .find(|p| p.id == params.id);

        match provider {
            Some(existing) => {
                // Persist the new API key (if provided) only after confirming the
                // provider exists — writing to the vault before this check would
                // leave an orphaned secret for a non-existent id.
                if let Some(api_key) = params.config.api_key.as_ref().filter(|k| !k.is_empty()) {
                    if let Err(e) = vault.store_secret(&vault_key(&params.id), api_key) {
                        error!(error = %e, "Failed to store embedding API key in vault");
                        return JsonRpcResponse::error(
                            request.id,
                            INTERNAL_ERROR,
                            format!("Failed to store API key: {e}"),
                        );
                    }
                }

                let mut new_config = params.config;
                new_config.verified = false;
                new_config.api_key = None; // Never persist to config
                apply_embedding_preset_defaults(&mut new_config);

                *existing = new_config;
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Embedding provider '{}' not found", params.id),
                );
            }
        }

        // Save to file (redact vault-backed api_keys)
        if let Err(e) = save_config(&cfg) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "updated",
        "provider_id": params.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Remove an embedding provider by id (reject if it's the active one).
pub async fn handle_remove(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg
            .memory
            .embedding
            .providers
            .iter()
            .any(|p| p.id == params.id)
        {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Embedding provider '{}' not found", params.id),
            );
        }

        // Reject if it's the active provider
        if cfg.memory.embedding.active_provider_id == params.id {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot remove provider '{}': it is the active embedding provider. Switch to another provider first.",
                    params.id
                ),
            );
        }

        // Remove provider
        cfg.memory.embedding.providers.retain(|p| p.id != params.id);

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.id)) {
            warn!(provider = %params.id, error = %e, "Failed to delete embedding API key from vault");
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["memory"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "removed",
        "provider_id": params.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Set a provider as active.
///
/// Multi-dimension vector columns allow different providers to coexist,
/// so switching providers does not require clearing the vector store.
pub async fn handle_set_active(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Check if provider exists and is verified
        match cfg
            .memory
            .embedding
            .providers
            .iter()
            .find(|p| p.id == params.id)
        {
            Some(provider) => {
                if !provider.verified {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!(
                            "Provider '{}' must pass a connection test before being set as default",
                            params.id
                        ),
                    );
                }
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Embedding provider '{}' not found", params.id),
                );
            }
        }

        cfg.memory.embedding.active_provider_id = params.id.clone();

        // Save to file
        if let Err(e) = cfg.save_incremental(&["memory"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.embedding.providers.changed",
        "action": "set_active",
        "provider_id": params.id,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Test an embedding provider's connectivity.
///
/// Creates a temporary `RemoteEmbeddingProvider` from the provided config and
/// calls `test_connection()` (embeds the word "test" and checks dimension match).
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Either a full config to test, or an id of an existing provider
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        config: Option<EmbeddingProviderConfig>,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Resolve the provider config to test
    let provider_config = if let Some(mut cfg) = params.config {
        // If inline config has no api_key, try resolving from vault
        if cfg.api_key.is_none() || cfg.api_key.as_deref() == Some("") {
            cfg.api_key = resolve_api_key(&cfg.id, &vault);
        }
        cfg
    } else if let Some(id) = params.id {
        let cfg = config.read().await;
        match cfg.memory.embedding.providers.iter().find(|p| p.id == id) {
            Some(p) => {
                let mut clone = p.clone();
                clone.api_key = resolve_api_key(&id, &vault);
                clone
            }
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Embedding provider '{id}' not found"),
                )
            }
        }
    } else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Either 'id' or 'config' must be provided",
        );
    };

    // Create a temporary provider and test connectivity
    let provider = match RemoteEmbeddingProvider::from_config(&provider_config) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "success": false,
                    "message": format!("Failed to create provider: {}", e),
                }),
            )
        }
    };

    match provider.test_connection().await {
        Ok(()) => {
            // Persist verified=true for the provider
            {
                let mut cfg = config.write().await;
                if let Some(p) = cfg
                    .memory
                    .embedding
                    .providers
                    .iter_mut()
                    .find(|p| p.id == provider_config.id)
                {
                    p.verified = true;
                    if let Err(e) = cfg.save_incremental(&["memory"]) {
                        tracing::error!(error = %e, "Failed to save config after embedding test");
                    }
                }
            }

            JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "success": true,
                    "message": format!(
                        "Connection successful — model '{}', {} dimensions",
                        provider_config.default_model(), provider_config.dimensions
                    ),
                }),
            )
        }
        Err(e) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "success": false,
                "message": format!("Connection test failed: {}", e),
            }),
        ),
    }
}
/// Return preset embedding provider configurations.
pub async fn handle_presets(request: JsonRpcRequest) -> JsonRpcResponse {
    let presets = vec![
        serde_json::json!({
            "preset": "silicon_flow",
            "id": "siliconflow",
            "name": "SiliconFlow",
            "api_base": "https://api.siliconflow.cn/v1",
            "api_key_env": null,
            "model": "BAAI/bge-m3",
            "dimensions": 1024
        }),
        serde_json::json!({
            "preset": "open_ai",
            "id": "openai",
            "name": "OpenAI",
            "api_base": "https://api.openai.com/v1",
            "api_key_env": null,
            "model": "text-embedding-3-small",
            "dimensions": 1536
        }),
        serde_json::json!({
            "preset": "ollama",
            "id": "ollama",
            "name": "Ollama",
            "api_base": "http://localhost:11434/v1",
            "api_key_env": null,
            "model": "nomic-embed-text",
            "dimensions": 768
        }),
        serde_json::json!({
            "preset": "jina",
            "id": "jina",
            "name": "Jina",
            "api_base": "https://api.jina.ai/v1",
            "api_key_env": null,
            "model": "jina-embeddings-v3",
            "dimensions": 1024
        }),
        serde_json::json!({
            "preset": "mistral",
            "id": "mistral",
            "name": "Mistral",
            "api_base": "https://api.mistral.ai/v1",
            "api_key_env": null,
            "model": "mistral-embed",
            "dimensions": 1024
        }),
    ];

    JsonRpcResponse::success(request.id, serde_json::json!(presets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    fn test_vault() -> Arc<SharedTokenManager> {
        use crate::gateway::security::SecurityStore;
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let tmp = std::env::temp_dir().join(format!("test_vault_{}.vault", std::process::id()));
        Arc::new(SharedTokenManager::new(store, tmp))
    }

    /// Build a Config with siliconflow added and set as active
    fn config_with_siliconflow() -> Config {
        use crate::config::types::memory::EmbeddingProviderConfig;
        let mut cfg = Config::default();
        cfg.memory
            .embedding
            .providers
            .push(EmbeddingProviderConfig::siliconflow());
        cfg.memory.embedding.active_provider_id = "siliconflow".to_string();
        cfg
    }

    #[tokio::test]
    async fn test_handle_list_empty_default() {
        let config = Arc::new(RwLock::new(Config::default()));
        let vault = test_vault();
        let request =
            JsonRpcRequest::with_id("embedding_providers.list", None, serde_json::json!(1));
        let response = handle_list(request, config, vault).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let providers = result.as_array().unwrap();
        assert_eq!(providers.len(), 0);
    }

    #[tokio::test]
    async fn test_handle_list_with_provider() {
        let config = Arc::new(RwLock::new(config_with_siliconflow()));
        let vault = test_vault();
        let request =
            JsonRpcRequest::with_id("embedding_providers.list", None, serde_json::json!(1));
        let response = handle_list(request, config, vault).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let providers = result.as_array().unwrap();
        assert_eq!(providers.len(), 1);
        let first = &providers[0];
        assert_eq!(first["id"].as_str().unwrap(), "siliconflow");
        assert!(first["is_active"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_get_found() {
        let config = Arc::new(RwLock::new(config_with_siliconflow()));
        let vault = test_vault();
        let request = JsonRpcRequest::with_id(
            "embedding_providers.get",
            Some(serde_json::json!({ "id": "siliconflow" })),
            serde_json::json!(1),
        );
        let response = handle_get(request, config, vault).await;
        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["id"].as_str().unwrap(), "siliconflow");
        assert!(result["is_active"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_get_not_found() {
        let config = Arc::new(RwLock::new(Config::default()));
        let vault = test_vault();
        let request = JsonRpcRequest::with_id(
            "embedding_providers.get",
            Some(serde_json::json!({ "id": "nonexistent" })),
            serde_json::json!(1),
        );
        let response = handle_get(request, config, vault).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_remove_rejects_active() {
        let config = Arc::new(RwLock::new(config_with_siliconflow()));
        let event_bus = Arc::new(GatewayEventBus::new());
        let vault = test_vault();
        let request = JsonRpcRequest::with_id(
            "embedding_providers.remove",
            Some(serde_json::json!({ "id": "siliconflow" })),
            serde_json::json!(1),
        );
        let response = handle_remove(request, config, event_bus, vault).await;
        assert!(response.is_error());
    }
}
