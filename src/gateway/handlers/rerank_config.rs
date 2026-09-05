//! Rerank Configuration RPC handlers
//!
//! Dedicated handlers for cross-encoder reranking provider configuration,
//! decoupled from the broader `memory_config` module.
//!
//! API keys are stored in the encrypted vault, never in config.toml.
//!
//! | Method | Description |
//! |--------|-------------|
//! | rerank_config.get | Get current rerank configuration |
//! | rerank_config.update | Update rerank configuration |
//! | rerank_config.test | Test rerank provider connectivity |

use serde_json::json;
use tracing::{error, info};

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
#[cfg(test)]
use crate::memory::rerank::RerankProviderType;
use crate::memory::rerank::{self, RerankConfig};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Build vault key for a rerank provider (consistent with other modules: prefix:name)
fn vault_key(provider_name: &str) -> String {
    format!("rerank:{provider_name}")
}

/// Get provider name as lowercase string
fn provider_name(provider: &rerank::RerankProviderType) -> String {
    serde_json::to_value(provider)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "jina".to_string())
}

/// Resolve API key from vault for a rerank provider
fn resolve_api_key(provider_name: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(provider_name), vault)
}

/// Handle `rerank_config.get` request
///
/// If params contains `"provider": "jina"`, reports whether that provider has a
/// vault key; otherwise checks the active provider. The stored secret is never
/// echoed — only a `has_api_key` flag is returned (security: 3def857c6).
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut rerank = serde_json::to_value(&cfg.memory.rerank).unwrap_or_else(|_| json!({}));

    // Determine which provider's key to inject
    let query_provider = request
        .params
        .as_ref()
        .and_then(|p| p.get("provider"))
        .and_then(|v| v.as_str())
        .map_or_else(|| provider_name(&cfg.memory.rerank.provider), String::from);

    // Security (3def857c6): never echo the stored secret. Report presence only
    // via `has_api_key` so the Panel can render its key-status hint; the editable
    // field always starts empty and an empty value on save means "keep existing".
    let has_api_key = resolve_api_key(&query_provider, &vault).is_some();
    if let Some(obj) = rerank.as_object_mut() {
        obj.remove("api_key");
        obj.insert("has_api_key".into(), json!(has_api_key));
    }

    JsonRpcResponse::success(request.id, rerank)
}

/// Handle `rerank_config.update` request
///
/// Stores API key in vault, saves remaining config to config.toml.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let mut rerank_config: RerankConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid rerank config: {e}"),
            )
        }
    };

    // Store API key in vault if provided (keyed by provider name)
    let provider_name = provider_name(&rerank_config.provider);
    if !rerank_config.api_key.is_empty() {
        if let Err(e) = vault.store_secret(&vault_key(&provider_name), &rerank_config.api_key) {
            error!(error = %e, "Failed to store rerank API key in vault");
        }
    }
    // Clear api_key before saving to config (it's in vault now)
    rerank_config.api_key = String::new();

    {
        let mut cfg = config.write().await;

        // Reset verified if provider changed
        let provider_changed = cfg.memory.rerank.provider != rerank_config.provider;
        if provider_changed {
            rerank_config.verified = false;
        } else {
            // Preserve existing verified status
            rerank_config.verified = cfg.memory.rerank.verified;
        }

        cfg.memory.rerank = rerank_config;

        if let Err(e) = cfg.save_incremental(&["memory.rerank"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("memory.rerank".to_string()),
        value: json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    info!("Rerank config updated via RPC");
    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

/// Handle `rerank_config.test` request
///
/// Builds a rerank provider from the supplied config and sends a test query.
/// If no `api_key` in request, falls back to vault.
/// On success, persists verified=true to config.
pub async fn handle_test(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(p) => p.clone(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let mut config: RerankConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid rerank config: {e}"),
            )
        }
    };

    // If no api_key in request, fall back to vault (keyed by provider)
    if config.api_key.is_empty() {
        let provider_name = provider_name(&config.provider);
        if let Some(key) = resolve_api_key(&provider_name, &vault) {
            config.api_key = key;
        }
    }

    let provider = rerank::build_provider(&config);

    let test_docs = vec![
        "The user prefers Rust programming language.".to_string(),
        "Today's weather is sunny and warm.".to_string(),
        "Memory optimization is an important task.".to_string(),
    ];

    match provider
        .rerank(
            "What programming language does the user prefer?",
            &test_docs,
            3,
        )
        .await
    {
        Ok(results) => {
            // Persist verified=true on success
            {
                let mut cfg = config_store.write().await;
                cfg.memory.rerank.verified = true;
                if let Err(e) = cfg.save_incremental(&["memory.rerank"]) {
                    error!(error = %e, "Failed to save rerank verified status");
                }
            }

            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": true,
                    "results_count": results.len(),
                    "top_score": results.first().map_or(0.0, |r| r.relevance_score),
                }),
            )
        }
        Err(e) => JsonRpcResponse::success(
            request.id,
            json!({
                "success": false,
                "error": e.to_string(),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn test_handle_get_default() {
        let config = Arc::new(RwLock::new(Config::default()));
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(store, "/tmp/test_rerank.vault"));
        let _ = vault.generate_token();

        let request = JsonRpcRequest::with_id("rerank_config.get", None, serde_json::json!(1));
        let response = handle_get(request, config, vault).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["enabled"].as_bool(), Some(false));
    }

    // Security (3def857c6): get reports `has_api_key` from the vault but never
    // echoes the secret back to the Panel.
    #[tokio::test]
    async fn test_handle_get_reports_has_api_key_without_echoing_secret() {
        let config = Arc::new(RwLock::new(Config::default()));
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            "/tmp/test_rerank_haskey.vault",
        ));
        let _ = vault.generate_token();
        // Active provider defaults to "jina"; seed its vault key.
        vault
            .store_secret(&vault_key("jina"), "super-secret-key")
            .unwrap();

        let request = JsonRpcRequest::with_id("rerank_config.get", None, serde_json::json!(1));
        let response = handle_get(request, config, vault).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["has_api_key"].as_bool(), Some(true));
        assert!(
            result.get("api_key").is_none(),
            "stored secret must never be echoed back"
        );
        // The serialized response must not contain the secret anywhere.
        assert!(!result.to_string().contains("super-secret-key"));
    }

    #[tokio::test]
    async fn test_handle_update_stores_key_in_vault() {
        // `handle_update` persists via `save_incremental`, whose path resolves
        // off `ALEPH_HOME`. Hold the crate-wide guard for the whole test (same
        // single source of mutual exclusion as the fetch/search/providers
        // handler tests): without it this test observed a concurrently-running
        // test's tempdir override — and, once that tempdir was dropped, failed
        // the save with "No such file or directory". The override also keeps
        // the write off the developer's real ~/.aleph/config.toml.
        let _home_guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("ALEPH_HOME").ok();
        // SAFETY: test holds the ALEPH_HOME guard; scoped env override, restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", dir.path());
        }

        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            "/tmp/test_rerank_update.vault",
        ));
        let _ = vault.generate_token();

        let params = json!({
            "enabled": true,
            "provider": "jina",
            "api_key": "test-secret-key",
            "model": "jina-reranker-v2-base-multilingual",
            "timeout_ms": 3000,
            "rerank_weight": 0.7,
        });

        let request =
            JsonRpcRequest::with_id("rerank_config.update", Some(params), serde_json::json!(1));
        let response = handle_update(request, config.clone(), event_bus, vault.clone()).await;
        assert!(response.is_success(), "update failed: {:?}", response.error);

        // Verify api_key is NOT in config
        let cfg = config.read().await;
        assert!(cfg.memory.rerank.enabled);
        assert_eq!(cfg.memory.rerank.provider, RerankProviderType::Jina);
        assert!(
            cfg.memory.rerank.api_key.is_empty(),
            "api_key should be empty in config"
        );

        // Verify api_key IS in vault
        let secret = vault.get_secret(&vault_key("jina")).unwrap().unwrap();
        assert_eq!(secret.expose(), "test-secret-key");

        match prev {
            // SAFETY: restoring previously-read env var while the ALEPH_HOME guard is held.
            Some(v) => unsafe { std::env::set_var("ALEPH_HOME", v) },
            // SAFETY: removing the env var we set above while the ALEPH_HOME guard is held.
            None => unsafe { std::env::remove_var("ALEPH_HOME") },
        }
    }

    #[tokio::test]
    async fn test_handle_update_invalid() {
        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            "/tmp/test_rerank_invalid.vault",
        ));
        let _ = vault.generate_token();

        let params = json!({ "provider": 42 });
        let request =
            JsonRpcRequest::with_id("rerank_config.update", Some(params), serde_json::json!(1));
        let response = handle_update(request, config, event_bus, vault).await;
        assert!(response.is_error());
    }

    #[test]
    fn test_update_preserves_other_memory_fields() {
        let mut cfg = Config::default();
        cfg.memory.enabled = true;
        cfg.memory.rrf_k = 42;
        cfg.memory.bm25_bonus_weight = 0.33;

        cfg.memory.rerank = RerankConfig {
            enabled: true,
            provider: RerankProviderType::Voyage,
            models: vec!["rerank-2".to_string()],
            ..Default::default()
        };

        assert!(cfg.memory.rerank.enabled);
        assert_eq!(cfg.memory.rerank.provider, RerankProviderType::Voyage);
        assert!(cfg.memory.enabled);
        assert_eq!(cfg.memory.rrf_k, 42);
        assert_eq!(cfg.memory.bm25_bonus_weight, 0.33);
    }
}
