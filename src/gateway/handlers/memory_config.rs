//! Memory Configuration Handlers
//!
//! RPC handlers for managing memory/RAG configuration:
//! - memory_config.get: Get current memory configuration
//! - memory_config.update: Update memory configuration
//! - memory.retrieve_with_trace: Retrieve memories with scoring trace (placeholder)
//!
//! All modifications are persisted to config file and broadcast as events.
//!
//! Note: Rerank configuration has its own dedicated handlers in `rerank_config`.

use serde_json::json;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Handle memory_config.get request
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;

    let memory_config = serde_json::to_value(&cfg.memory).unwrap_or_else(|_| serde_json::json!({}));

    JsonRpcResponse::success(request.id, memory_config)
}

/// Handle memory_config.update request
///
/// Uses JSON merge to update only the fields provided by the caller,
/// preserving any fields not present in the incoming payload (e.g.
/// embedding, scoring_pipeline, adaptive_retrieval, noise_filter).
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params as raw JSON value
    let incoming = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    // Merge: read existing config as JSON, overlay incoming fields, deserialize back
    {
        let mut cfg = config.write().await;

        // Serialize existing memory config to JSON
        let mut base = match serde_json::to_value(&cfg.memory) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize existing config: {}", e),
                )
            }
        };

        // Merge incoming fields on top of existing (only overwrites keys present in incoming)
        json_merge(&mut base, &incoming);

        // Deserialize merged JSON back to MemoryConfig
        let merged: crate::config::types::memory::MemoryConfig = match serde_json::from_value(base)
        {
            Ok(c) => c,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid memory config after merge: {}", e),
                )
            }
        };

        cfg.memory = merged;

        // Save to file
        if let Err(e) = cfg.save_incremental(&["memory"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
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
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'query' parameter");
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

// ============================================================================
// Helpers
// ============================================================================

/// Recursively merge `overlay` into `base`.
/// For objects, overlay keys overwrite base keys; for all other types the
/// overlay value replaces the base value entirely.
fn json_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map.entry(key.clone()).or_insert(Value::Null);
                json_merge(entry, overlay_val);
            }
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}
