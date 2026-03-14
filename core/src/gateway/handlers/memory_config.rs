//! Memory Configuration Handlers
//!
//! RPC handlers for managing memory/RAG configuration:
//! - memory_config.get: Get current memory configuration
//! - memory_config.update: Update memory configuration
//! - memory.test_rerank_connection: Test rerank provider connectivity
//! - memory.retrieve_with_trace: Retrieve memories with scoring trace (placeholder)
//!
//! All modifications are persisted to config file and broadcast as events.

use serde_json::json;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::event_bus::{GatewayEventBus, GatewayEvent, ConfigChangedEvent};
use crate::memory::rerank::{self, RerankConfig};

/// Handle memory_config.get request
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let memory_config = serde_json::to_value(&cfg.memory)
        .unwrap_or_else(|_| serde_json::json!({}));

    JsonRpcResponse::success(request.id, memory_config)
}

/// Handle memory_config.update request
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let memory_config: crate::config::types::memory::MemoryConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid memory config: {}", e)),
    };

    // Update config
    {
        let mut cfg = config.write().await;
        cfg.memory = memory_config;

        // Save to file
        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to save config: {}", e));
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("memory".to_string()),
        value: serde_json::json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

// ============================================================================
// Rerank Connection Test
// ============================================================================

/// Handle memory.test_rerank_connection request
///
/// Builds a rerank provider from the supplied config and sends a test query
/// with 3 sample documents. Returns success/failure with score info.
pub async fn handle_test_rerank_connection(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(p) => p.clone(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let config: RerankConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid rerank config: {}", e),
            )
        }
    };

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
        Ok(results) => JsonRpcResponse::success(
            request.id,
            json!({
                "success": true,
                "results_count": results.len(),
                "top_score": results.first().map(|r| r.relevance_score).unwrap_or(0.0),
            }),
        ),
        Err(e) => JsonRpcResponse::success(
            request.id,
            json!({
                "success": false,
                "error": e.to_string(),
            }),
        ),
    }
}

// ============================================================================
// Retrieve with Trace (placeholder)
// ============================================================================

/// Handle memory.retrieve_with_trace request
///
/// Placeholder — full wiring requires the memory service. Returns a mock trace
/// to validate the RPC registration works.
pub async fn handle_retrieve_with_trace(request: JsonRpcRequest) -> JsonRpcResponse {
    let query = request
        .params
        .as_ref()
        .and_then(|p| p["query"].as_str())
        .unwrap_or("");

    if query.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Missing 'query' parameter",
        );
    }

    let trace = crate::memory::retrieval_trace::RetrievalTrace::new(query, 0);

    JsonRpcResponse::success(
        request.id,
        json!({
            "query": query,
            "trace": trace,
            "results": [],
            "status": "placeholder — full wiring pending",
        }),
    )
}
