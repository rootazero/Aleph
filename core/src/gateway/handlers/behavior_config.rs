//! Behavior configuration RPC handlers
//!
//! Provides RPC methods for managing behavior configuration (output mode, typing speed).

use crate::config::{BehaviorConfig, Config};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfigDto {
    pub output_mode: String,
    pub typing_speed: u32,
}

/// Get behavior configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let default_behavior = BehaviorConfig::default();
    let behavior = cfg.behavior.as_ref().unwrap_or(&default_behavior);

    let dto = BehaviorConfigDto {
        output_mode: behavior.output_mode.clone(),
        typing_speed: behavior.typing_speed,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
}

/// Update behavior configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    let dto: BehaviorConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    // Validate output_mode
    if dto.output_mode != "typewriter" && dto.output_mode != "instant" {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "output_mode must be 'typewriter' or 'instant'".to_string(),
        );
    }

    // Validate typing_speed
    if dto.typing_speed < 50 || dto.typing_speed > 400 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "typing_speed must be between 50 and 400".to_string(),
        );
    }

    {
        let mut cfg = config.write().await;

        // Initialize behavior if None
        if cfg.behavior.is_none() {
            cfg.behavior = Some(BehaviorConfig::default());
        }

        if let Some(behavior) = &mut cfg.behavior {
            behavior.output_mode = dto.output_mode.clone();
            behavior.typing_speed = dto.typing_speed;
        }

        if let Err(e) = cfg.save() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("behavior".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
