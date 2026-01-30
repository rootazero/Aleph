//! Configuration Handlers
//!
//! RPC handlers for configuration operations: reload, get, validate.

use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, info};

use crate::gateway::hot_reload::ConfigWatcher;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

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
}
