//! Behavior configuration RPC handlers
//!
//! Provides RPC methods for managing behavior configuration — `output_mode`
//! and nothing else. `typing_speed` was retired in the 2026-08-17 wire audit
//! (config-003, see `config::types::general`), so this file stopped sending it;
//! the sentence that used to name it here outlived the field.
//!
//! ⚠️ The Panel's DTO did NOT stop asking for it: `interfaces/webchat`'s
//! `api::settings::BehaviorConfig` still declares a `typing_speed: u32` with no
//! `#[serde(default)]`, so `behavior_config.get` fails to decode there on every
//! call — the boot fetch that sets the typewriter speed silently never lands
//! and the Behavior settings page always renders its load-error arm. Fixing it
//! is a product decision (server-owned knob ⇒ restore the field and a
//! `[behavior]` key; Panel-local preference ⇒ drop it from the client DTO), and
//! adding `#[serde(default)]` is NOT the fix — that pins the slider to a
//! default forever while still looking wired.

use crate::config::{BehaviorConfig, Config};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BehaviorConfigDto {
    pub output_mode: String,
}

/// Get behavior configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    let default_behavior = BehaviorConfig::default();
    let behavior = cfg.behavior.as_ref().unwrap_or(&default_behavior);

    let dto = BehaviorConfigDto {
        output_mode: behavior.output_mode.clone(),
    };

    match serde_json::to_value(dto) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
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
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params".to_string())
        }
    };

    let dto: BehaviorConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
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

    // Validate output_mode (already above).
    {
        let mut cfg = config.write().await;

        // Initialize behavior if None
        if cfg.behavior.is_none() {
            cfg.behavior = Some(BehaviorConfig::default());
        }

        if let Some(behavior) = &mut cfg.behavior {
            behavior.output_mode = dto.output_mode.clone();
        }

        if let Err(e) = cfg.save_incremental(&["behavior"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("behavior".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
