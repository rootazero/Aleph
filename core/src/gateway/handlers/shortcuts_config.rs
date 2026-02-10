//! Shortcuts configuration RPC handlers
//!
//! Provides RPC methods for managing keyboard shortcuts configuration.

use crate::config::{Config, ShortcutsConfig};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfigDto {
    pub summon: String,
    pub cancel: Option<String>,
    pub command_prompt: String,
}

/// Get shortcuts configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    let shortcuts = cfg.shortcuts.as_ref().unwrap_or(&ShortcutsConfig::default());

    let dto = ShortcutsConfigDto {
        summon: shortcuts.summon.clone(),
        cancel: shortcuts.cancel.clone(),
        command_prompt: shortcuts.command_prompt.clone(),
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
}

/// Update shortcuts configuration
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

    let dto: ShortcutsConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    {
        let mut cfg = config.write().await;

        // Initialize shortcuts if None
        if cfg.shortcuts.is_none() {
            cfg.shortcuts = Some(ShortcutsConfig::default());
        }

        if let Some(shortcuts) = &mut cfg.shortcuts {
            shortcuts.summon = dto.summon;
            shortcuts.cancel = dto.cancel;
            shortcuts.command_prompt = dto.command_prompt;
        }

        if let Err(e) = cfg.save_to_file() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast config change event
    let _ = event_bus.publish(
        "config.shortcuts.changed",
        serde_json::json!({ "timestamp": chrono::Utc::now().timestamp_millis() }),
    );

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
