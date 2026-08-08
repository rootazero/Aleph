//! Execution engine configuration RPC handlers
//!
//! Provides RPC methods for managing agent execution settings (timeout, iterations).

use crate::config::types::ExecutionConfig;
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;

/// Get execution configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    match serde_json::to_value(&cfg.execution) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

/// Update execution configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let update: ExecutionConfig = match serde_json::from_value(params) {
        Ok(u) => u,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    // Validate ranges
    if update.default_timeout_secs < 60 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "default_timeout_secs must be at least 60 (1 minute)",
        );
    }
    if update.default_timeout_secs > 604_800 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "default_timeout_secs must be at most 604800 (7 days)",
        );
    }
    if update.max_iterations < 5 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "max_iterations must be at least 5",
        );
    }
    if update.max_iterations > 10_000 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "max_iterations must be at most 10000",
        );
    }

    {
        let mut cfg = config.write().await;
        cfg.execution = update.clone();

        if let Err(e) = cfg.save_incremental(&["execution"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("execution".to_string()),
        value: serde_json::to_value(&update).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
