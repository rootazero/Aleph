//! Memory Configuration Handlers
//!
//! RPC handlers for managing memory/RAG configuration:
//! - memory_config.get: Get current memory configuration
//! - memory_config.update: Update memory configuration
//!
//! All modifications are persisted to config file and broadcast as events.

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::event_bus::{GatewayEventBus, GatewayEvent, ConfigChangedEvent};

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
