//! Security Configuration Handlers
//!
//! RPC handlers for managing security settings:
//! - security_config.get: Get current security configuration
//! - security_config.update: Update security configuration
//! - security_config.list_devices: List all paired devices
//! - security_config.revoke_device: Revoke a device's access
//!
//! All modifications are persisted and broadcast as events.

use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::event_bus::{GatewayEventBus, GatewayEvent, ConfigChangedEvent};
use crate::gateway::device_store::DeviceStore;

/// Security configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Require authentication for Gateway connections
    pub require_auth: bool,
    /// Enable device pairing
    pub enable_pairing: bool,
    /// Allow guest access
    pub allow_guest: bool,
}

/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

/// Handle security_config.get request
pub async fn handle_get(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    // For now, return a simple security config
    // In a real implementation, this would read from Gateway config
    let security_config = SecurityConfig {
        require_auth: false,
        enable_pairing: true,
        allow_guest: false,
    };

    let result = serde_json::to_value(&security_config)
        .unwrap_or_else(|_| serde_json::json!({}));

    JsonRpcResponse::success(request.id, result)
}

/// Handle security_config.update request
pub async fn handle_update(
    request: JsonRpcRequest,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let _security_config: SecurityConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid security config: {}", e)),
    };

    // TODO: In a real implementation, update Gateway config
    // For now, just broadcast the event

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("security".to_string()),
        value: serde_json::json!({ "action": "updated" }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Handle security_config.list_devices request
pub async fn handle_list_devices(
    request: JsonRpcRequest,
    device_store: Arc<DeviceStore>,
) -> JsonRpcResponse {
    let devices = device_store.list_devices();

    let device_infos: Vec<DeviceInfo> = devices
        .into_iter()
        .map(|d| DeviceInfo {
            device_id: d.device_id,
            device_name: d.device_name,
            device_type: d.device_type.unwrap_or_else(|| "unknown".to_string()),
            paired_at: d.approved_at,
            last_seen: d.last_seen_at,
        })
        .collect();

    let result = serde_json::to_value(&device_infos)
        .unwrap_or_else(|_| serde_json::json!([]));

    JsonRpcResponse::success(request.id, result)
}

/// Handle security_config.revoke_device request
pub async fn handle_revoke_device(
    request: JsonRpcRequest,
    device_store: Arc<DeviceStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let device_id: String = match serde_json::from_value(params.get("device_id").cloned().unwrap_or(Value::Null)) {
        Ok(id) => id,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid device_id: {}", e)),
    };

    match device_store.revoke_device(&device_id) {
        Ok(_) => {
            // Broadcast event
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("security".to_string()),
                value: serde_json::json!({ "action": "device_revoked", "device_id": device_id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
        }
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to revoke device: {}", e))
        }
    }
}
