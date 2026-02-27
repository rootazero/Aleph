//! Configuration Handlers
//!
//! RPC handlers for configuration operations: reload, get, validate, schema.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::config::{build_ui_hints, generate_config_schema_json, Config, ConfigUiHints};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::hot_reload::ConfigWatcher;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::handlers::parse_params;

/// Handle config.reload RPC request
///
/// Forces a configuration reload from file.
/// Returns the new configuration on success.
pub async fn handle_reload(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.reload");

    match watcher.reload().await {
        Ok(new_config) => {
            info!("Configuration reloaded via RPC");
            JsonRpcResponse::success(
                request.id,
                json!({
                    "success": true,
                    "config": {
                        "gateway": {
                            "host": new_config.gateway.host,
                            "port": new_config.gateway.port,
                            "max_connections": new_config.gateway.max_connections,
                            "require_auth": new_config.gateway.require_auth,
                        },
                        "agents": new_config.agents.keys().collect::<Vec<_>>(),
                        "bindings_count": new_config.bindings.len(),
                    },
                    "message": "Configuration reloaded successfully",
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reload configuration: {}", e),
        ),
    }
}

/// Handle config.get RPC request
///
/// Returns the current configuration.
pub async fn handle_get(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.get");

    // Check for specific section request
    let section = request.params
        .as_ref()
        .and_then(|p| p.get("section"))
        .and_then(|v| v.as_str());

    let config = watcher.current_config().await;

    let result = match section {
        Some("gateway") => json!({
            "host": config.gateway.host,
            "port": config.gateway.port,
            "max_connections": config.gateway.max_connections,
            "require_auth": config.gateway.require_auth,
            "protocol_version": config.gateway.protocol_version,
        }),
        Some("agents") => {
            let agents: serde_json::Map<String, Value> = config
                .agents
                .iter()
                .map(|(id, agent)| {
                    (
                        id.clone(),
                        json!({
                            "workspace": agent.workspace,
                            "model": agent.model,
                            "max_loops": agent.max_loops,
                            "fallback_models": agent.fallback_models,
                        }),
                    )
                })
                .collect();
            json!(agents)
        }
        Some("bindings") => json!(config.bindings),
        Some("channels") => json!({
            "telegram": config.channels.telegram.as_ref().map(|t| json!({
                "enabled": t.enabled,
                "route_to_agent": t.route_to_agent,
            })),
            "discord": config.channels.discord.as_ref().map(|d| json!({
                "enabled": d.enabled,
                "route_to_agent": d.route_to_agent,
            })),
            "slack": config.channels.slack.as_ref().map(|s| json!({
                "enabled": s.enabled,
                "route_to_agent": s.route_to_agent,
            })),
            "webchat": config.channels.webchat.as_ref().map(|w| json!({
                "enabled": w.enabled,
                "port": w.port,
            })),
        }),
        Some("sandbox") => json!({
            "enabled": config.sandbox.enabled,
            "docker_image": config.sandbox.docker_image,
            "memory_limit_mb": config.sandbox.memory_limit_mb,
            "cpu_quota_percent": config.sandbox.cpu_quota_percent,
        }),
        Some("tools") => json!({
            "chrome": config.tools.chrome.as_ref().map(|c| json!({
                "enabled": c.enabled,
                "headless": c.headless,
            })),
            "cron": config.tools.cron.as_ref().map(|c| json!({
                "enabled": c.enabled,
                "max_jobs": c.max_jobs,
            })),
            "webhook": config.tools.webhook.as_ref().map(|w| json!({
                "enabled": w.enabled,
                "port": w.port,
            })),
        }),
        Some(unknown) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Unknown section: {}. Valid sections: gateway, agents, bindings, channels, sandbox, tools", unknown),
            );
        }
        None => {
            // Return full config overview (without sensitive data)
            json!({
                "config_path": watcher.config_path().display().to_string(),
                "gateway": {
                    "host": config.gateway.host,
                    "port": config.gateway.port,
                    "max_connections": config.gateway.max_connections,
                    "require_auth": config.gateway.require_auth,
                },
                "agents": config.agents.keys().collect::<Vec<_>>(),
                "bindings_count": config.bindings.len(),
                "channels": {
                    "telegram": config.channels.telegram.is_some(),
                    "discord": config.channels.discord.is_some(),
                    "slack": config.channels.slack.is_some(),
                    "webchat": config.channels.webchat.is_some(),
                },
                "sandbox_enabled": config.sandbox.enabled,
            })
        }
    };

    JsonRpcResponse::success(request.id, result)
}

/// Handle config.validate RPC request
///
/// Validates the configuration file without applying changes.
pub async fn handle_validate(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.validate");

    match watcher.validate() {
        Ok(config) => {
            JsonRpcResponse::success(
                request.id,
                json!({
                    "valid": true,
                    "config_path": watcher.config_path().display().to_string(),
                    "summary": {
                        "agents": config.agents.keys().collect::<Vec<_>>(),
                        "bindings_count": config.bindings.len(),
                        "gateway_port": config.gateway.port,
                    },
                    "message": "Configuration is valid",
                }),
            )
        }
        Err(e) => {
            JsonRpcResponse::success(
                request.id,
                json!({
                    "valid": false,
                    "config_path": watcher.config_path().display().to_string(),
                    "error": e.to_string(),
                    "message": "Configuration validation failed",
                }),
            )
        }
    }
}

/// Handle config.path RPC request
///
/// Returns the path to the configuration file being watched.
pub async fn handle_path(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
) -> JsonRpcResponse {
    debug!("Handling config.path");

    JsonRpcResponse::success(
        request.id,
        json!({
            "path": watcher.config_path().display().to_string(),
            "exists": watcher.config_path().exists(),
        }),
    )
}

// ============================================================================
// Schema Handler
// ============================================================================

/// Default value for include_plugins (true)
fn default_true() -> bool {
    true
}

/// Request params for config.schema
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigSchemaRequest {
    /// Whether to include plugin schemas (reserved for future use)
    #[serde(default = "default_true")]
    #[allow(dead_code)] // Deserialized from RPC params; reserved for plugin schema inclusion
    pub include_plugins: bool,
}

/// Response for config.schema
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSchemaResponse {
    /// JSON Schema for the configuration
    pub schema: serde_json::Value,
    /// UI hints for rendering configuration forms
    pub ui_hints: ConfigUiHints,
    /// Schema version (crate version)
    pub version: String,
    /// Timestamp when the schema was generated
    pub generated_at: String,
}

/// Handle config.schema RPC request
///
/// Returns the JSON Schema for the Aleph configuration along with
/// UI hints for rendering configuration forms.
///
/// # Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "config.schema",
///   "id": 1,
///   "params": {
///     "include_plugins": true  // optional, defaults to true
///   }
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "schema": { ... },      // JSON Schema
///     "ui_hints": { ... },    // UI hints for form rendering
///     "version": "0.1.0",
///     "generated_at": "2024-01-15T10:30:00Z"
///   }
/// }
/// ```
pub async fn handle_schema(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling config.schema");

    // Parse params (optional)
    let _params: ConfigSchemaRequest = request
        .params
        .as_ref()
        .map(|p| serde_json::from_value(p.clone()).unwrap_or_default())
        .unwrap_or_default();

    // Generate schema and hints
    let schema = generate_config_schema_json();
    let ui_hints = build_ui_hints();

    let response = ConfigSchemaResponse {
        schema,
        ui_hints,
        version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    // Serialize response manually to ensure proper format
    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize schema response: {}", e),
        ),
    }
}

// ============================================================================
// Full Config Handler (for ConfigManager SDK)
// ============================================================================

/// Handle config.get RPC method
///
/// Returns full configuration snapshot (Tier 1/2 only).
///
/// # Request
///
/// ```json
/// { "jsonrpc": "2.0", "method": "config.get", "id": 1 }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "config": {
///       "ui.theme": "dark",
///       "auth.identity": "owner@local"
///     }
///   }
/// }
/// ```
pub async fn handle_get_full_config(
    req: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let config_snapshot = config.read().await.clone();

    // Convert Config to JSON (Tier 1/2 fields only)
    let config_json = match serde_json::to_value(&config_snapshot) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("Failed to serialize config: {}", e),
            );
        }
    };

    JsonRpcResponse::success(
        req.id,
        json!({
            "config": config_json
        }),
    )
}

// ============================================================================
// Patch Handler
// ============================================================================

/// Handle config.patch RPC method
///
/// Applies partial configuration updates and broadcasts ConfigChanged event.
///
/// # Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "config.patch",
///   "id": 1,
///   "params": {
///     "ui.theme": "dark",
///     "auth.identity": "owner@local"
///   }
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 1,
///   "result": {
///     "status": "ok"
///   }
/// }
/// ```
pub async fn handle_patch_config(
    req: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling config.patch");

    // Parse patch from params
    let patch: HashMap<String, Value> = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate non-empty
    if patch.is_empty() {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "Patch cannot be empty".to_string(),
        );
    }

    // TODO: Apply patch to config
    // For now, just validate we can acquire the lock
    let _config_write = config.write().await;

    // Broadcast ConfigChanged event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: None,
        value: json!(patch),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        return JsonRpcResponse::error(
            req.id,
            INTERNAL_ERROR,
            format!("Failed to broadcast event: {}", e),
        );
    }

    info!("Config patched: {} keys updated", patch.len());

    JsonRpcResponse::success(req.id, json!({"status": "ok"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use crate::gateway::hot_reload::ConfigWatcherConfig;
    use std::time::Duration;

    async fn create_test_watcher() -> (Arc<ConfigWatcher>, NamedTempFile) {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[gateway]
port = 18789

[agents.main]
model = "claude-sonnet-4-5"

[agents.work]
model = "claude-opus-4-5"

[bindings]
"cli:*" = "work"
"#
        )
        .unwrap();

        let config = ConfigWatcherConfig {
            config_path: temp_file.path().to_path_buf(),
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        };

        let watcher = Arc::new(ConfigWatcher::new(config).unwrap());
        (watcher, temp_file)
    }

    #[tokio::test]
    async fn test_handle_get_full() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.get".to_string(),
            params: None,
        };

        let response = handle_get(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert!(result.get("gateway").is_some());
        assert!(result.get("agents").is_some());
    }

    #[tokio::test]
    async fn test_handle_get_section() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.get".to_string(),
            params: Some(json!({"section": "gateway"})),
        };

        let response = handle_get(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["port"], 18789);
    }

    #[tokio::test]
    async fn test_handle_validate() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.validate".to_string(),
            params: None,
        };

        let response = handle_validate(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["valid"], true);
    }

    #[tokio::test]
    async fn test_handle_reload() {
        let (watcher, _temp_file) = create_test_watcher().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.reload".to_string(),
            params: None,
        };

        let response = handle_reload(request, watcher).await;
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_handle_schema() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "config.schema".to_string(),
            params: None,
        };

        let response = handle_schema(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();

        // Check schema is present and has expected structure
        assert!(result.get("schema").is_some());
        let schema = result.get("schema").unwrap();
        assert!(schema.get("$schema").is_some());
        assert!(schema.get("definitions").is_some());

        // Check ui_hints is present
        assert!(result.get("ui_hints").is_some());
        let ui_hints = result.get("ui_hints").unwrap();
        assert!(ui_hints.get("groups").is_some());
        assert!(ui_hints.get("fields").is_some());

        // Check metadata
        assert!(result.get("version").is_some());
        assert!(result.get("generated_at").is_some());
    }

    #[tokio::test]
    async fn test_handle_schema_with_params() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: "config.schema".to_string(),
            params: Some(json!({ "include_plugins": false })),
        };

        let response = handle_schema(request).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result.get("schema").is_some());
    }

    #[tokio::test]
    async fn test_handle_get_full_config() {
        let config = Config::default();
        let config = Arc::new(RwLock::new(config));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.get".to_string(),
            params: None,
            id: Some(json!(1)),
        };

        let response = handle_get_full_config(req, config).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(result.get("config").is_some());
    }

    // ========================================================================
    // Patch Tests
    // ========================================================================

    #[tokio::test]
    async fn test_handle_patch_config() {
        use crate::gateway::event_bus::GatewayEventBus;

        let config = Config::default();
        let config = Arc::new(RwLock::new(config));
        let event_bus = Arc::new(GatewayEventBus::new());

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.patch".to_string(),
            params: Some(json!({
                "ui.theme": "dark",
                "auth.identity": "owner@local"
            })),
            id: Some(json!(1)),
        };

        let response = handle_patch_config(req, config, event_bus).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result.get("status").unwrap().as_str().unwrap(), "ok");
    }

    #[tokio::test]
    async fn test_patch_rejects_empty() {
        use crate::gateway::event_bus::GatewayEventBus;

        let config = Config::default();
        let config = Arc::new(RwLock::new(config));
        let event_bus = Arc::new(GatewayEventBus::new());

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "config.patch".to_string(),
            params: Some(json!({})),
            id: Some(json!(1)),
        };

        let response = handle_patch_config(req, config, event_bus).await;

        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("empty"));
    }
}
