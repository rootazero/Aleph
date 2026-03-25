//! Security Configuration Handlers
//!
//! RPC handlers for managing security settings:
//! - security_config.get: Get current security configuration
//! - security_config.update: Update security configuration
//! - security_config.list_devices: List all paired devices
//! - security_config.revoke_device: Revoke a device's access
//!
//! All modifications are persisted and broadcast as events.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::event_bus::{GatewayEventBus, GatewayEvent, ConfigChangedEvent};
use crate::gateway::device_store::DeviceStore;
use crate::config::patcher::ConfigPatcher;

/// Write gateway.host to the config TOML file on disk.
fn write_gateway_host_to_config(path: &std::path::Path, host: &str) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table = toml::from_str(&contents)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    // Create or update [gateway] section
    let gateway = doc
        .entry("gateway".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(t) = gateway {
        t.insert("host".to_string(), toml::Value::String(host.to_string()));
    }

    let new_contents = toml::to_string_pretty(&doc)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;

    // Atomic write
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, &new_contents)
        .map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path)
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            format!("Failed to rename: {e}")
        })?;

    Ok(())
}

/// Read gateway.host from the config TOML file on disk.
fn read_gateway_host_from_config(patcher: &ConfigPatcher) -> String {
    // Read the config file as TOML and extract gateway.host
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(gateway) = table.get("gateway").and_then(|v| v.as_table()) {
                if let Some(host) = gateway.get("host").and_then(|v| v.as_str()) {
                    return host.to_string();
                }
            }
        }
    }
    "127.0.0.1".to_string() // default
}

/// Network access scope for gateway binding
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkAccess {
    /// Localhost only (127.0.0.1) — most secure
    Localhost,
    /// All network interfaces (0.0.0.0) — accessible from any network
    AllNetworks,
}

impl NetworkAccess {
    pub fn to_bind_address(&self) -> &str {
        match self {
            Self::Localhost => "127.0.0.1",
            Self::AllNetworks => "0.0.0.0",
        }
    }

    pub fn from_bind_address(addr: &str) -> Self {
        if addr == "0.0.0.0" || addr == "::" {
            Self::AllNetworks
        } else {
            Self::Localhost
        }
    }
}

/// Security configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Require authentication for Gateway connections
    pub require_auth: bool,
    /// Enable device pairing
    pub enable_pairing: bool,
    /// Allow guest access
    pub allow_guest: bool,
    /// Network access scope (localhost or lan)
    #[serde(default = "default_network_access")]
    pub network_access: NetworkAccess,
}

fn default_network_access() -> NetworkAccess {
    NetworkAccess::Localhost
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
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    // Read gateway.host from config file to determine network access scope
    let host = read_gateway_host_from_config(&config_patcher);

    let security_config = SecurityConfig {
        require_auth: false,
        enable_pairing: true,
        allow_guest: false,
        network_access: NetworkAccess::from_bind_address(&host),
    };

    let result = serde_json::to_value(&security_config)
        .unwrap_or_else(|_| serde_json::json!({}));

    JsonRpcResponse::success(request.id, result)
}

/// Handle security_config.update request
pub async fn handle_update(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let security_config: SecurityConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid security config: {}", e)),
    };

    // Check current host to determine if restart is needed
    let current_host = read_gateway_host_from_config(&config_patcher);

    let new_host = security_config.network_access.to_bind_address().to_string();
    let needs_restart = current_host != new_host;

    // Persist gateway.host directly to TOML (cannot use ConfigPatcher because
    // Config struct has no `gateway` field — the patcher would discard it).
    if needs_restart {
        let config_path = crate::config::Config::default_path();
        if let Err(e) = write_gateway_host_to_config(&config_path, &new_host) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("security".to_string()),
        value: serde_json::json!({
            "action": "updated",
            "needs_restart": needs_restart,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({
        "success": true,
        "needs_restart": needs_restart,
    }))
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
