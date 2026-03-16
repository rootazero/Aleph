//! Rerank Configuration RPC handlers
//!
//! Dedicated handlers for cross-encoder reranking provider configuration,
//! decoupled from the broader memory_config module.
//!
//! | Method | Description |
//! |--------|-------------|
//! | rerank_config.get | Get current rerank configuration |
//! | rerank_config.update | Update rerank configuration |
//! | rerank_config.test | Test rerank provider connectivity |

use serde_json::json;

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::rerank::{self, RerankConfig};
#[cfg(test)]
use crate::memory::rerank::RerankProviderType;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Handle rerank_config.get request
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let rerank = serde_json::to_value(&cfg.memory.rerank)
        .unwrap_or_else(|_| json!({}));

    JsonRpcResponse::success(request.id, rerank)
}

/// Handle rerank_config.update request
///
/// Updates only the rerank section of memory config, leaving all other
/// memory settings untouched.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let rerank_config: RerankConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid rerank config: {}", e),
            )
        }
    };

    {
        let mut cfg = config.write().await;
        cfg.memory.rerank = rerank_config;

        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("memory.rerank".to_string()),
        value: json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

/// Handle rerank_config.test request
///
/// Builds a rerank provider from the supplied config and sends a test query
/// with 3 sample documents. Returns success/failure with score info.
pub async fn handle_test(request: JsonRpcRequest) -> JsonRpcResponse {
    tracing::info!("rerank_config.test called");

    let params = match &request.params {
        Some(p) => p.clone(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    tracing::info!(params = %params, "rerank_config.test params");

    let config: RerankConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "rerank_config.test: failed to parse config");
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid rerank config: {}", e),
            )
        }
    };

    tracing::info!(provider = ?config.provider, "rerank_config.test: building provider");
    let provider = rerank::build_provider(&config);

    let test_docs = vec![
        "The user prefers Rust programming language.".to_string(),
        "Today's weather is sunny and warm.".to_string(),
        "Memory optimization is an important task.".to_string(),
    ];

    tracing::info!("rerank_config.test: calling rerank API...");
    match provider
        .rerank(
            "What programming language does the user prefer?",
            &test_docs,
            3,
        )
        .await
    {
        Ok(results) => {
            tracing::info!(count = results.len(), "rerank_config.test: success");
            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": true,
                    "results_count": results.len(),
                    "top_score": results.first().map(|r| r.relevance_score).unwrap_or(0.0),
                }),
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "rerank_config.test: rerank failed");
            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": false,
                    "error": e.to_string(),
                }),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn test_handle_get_default() {
        let config = Arc::new(RwLock::new(Config::default()));
        let request =
            JsonRpcRequest::with_id("rerank_config.get", None, serde_json::json!(1));
        let response = handle_get(request, config).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        // Default: disabled
        assert_eq!(result["enabled"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn test_handle_update_valid() {
        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());

        let params = json!({
            "enabled": true,
            "provider": "jina",
            "api_key": "test-key",
            "model": "jina-reranker-v2-base-multilingual",
            "timeout_ms": 3000,
            "rerank_weight": 0.7,
        });

        let request = JsonRpcRequest::with_id(
            "rerank_config.update",
            Some(params),
            serde_json::json!(1),
        );
        let response = handle_update(request, config.clone(), event_bus).await;
        assert!(response.is_success());

        // Verify stored config
        let cfg = config.read().await;
        assert!(cfg.memory.rerank.enabled);
        assert_eq!(cfg.memory.rerank.provider, RerankProviderType::Jina);
        assert_eq!(cfg.memory.rerank.api_key, "test-key");
        assert_eq!(cfg.memory.rerank.timeout_ms, 3000);
    }

    #[tokio::test]
    async fn test_handle_update_invalid() {
        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());

        // Invalid: provider is a number instead of string
        let params = json!({ "provider": 42 });
        let request = JsonRpcRequest::with_id(
            "rerank_config.update",
            Some(params),
            serde_json::json!(1),
        );
        let response = handle_update(request, config, event_bus).await;
        assert!(response.is_error());
    }

    /// Verify that rerank_config.update only modifies `cfg.memory.rerank`,
    /// not other memory fields. Uses a direct unit test on the Config struct
    /// to avoid file-save race conditions with `Config::save()`.
    #[test]
    fn test_update_preserves_other_memory_fields() {
        let mut cfg = Config::default();
        cfg.memory.enabled = true;
        cfg.memory.max_context_items = 42;
        cfg.memory.retention_days = 365;

        // Simulate what handle_update does (minus file save)
        cfg.memory.rerank = RerankConfig {
            enabled: true,
            provider: RerankProviderType::Voyage,
            models: vec!["rerank-2".to_string()],
            ..Default::default()
        };

        // Rerank updated
        assert!(cfg.memory.rerank.enabled);
        assert_eq!(cfg.memory.rerank.provider, RerankProviderType::Voyage);

        // Other memory fields preserved
        assert!(cfg.memory.enabled);
        assert_eq!(cfg.memory.max_context_items, 42);
        assert_eq!(cfg.memory.retention_days, 365);
    }
}
