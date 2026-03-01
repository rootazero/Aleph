//! MCP Configuration RPC Handlers
//!
//! Handlers for MCP server configuration management: list, create, update, delete.
//! These handlers manage MCP server configurations in the config file.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, McpServerConfig};

// ============================================================================
// Types
// ============================================================================

/// MCP server info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct McpServerInfo {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// MCP server config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfigJson {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub requires_runtime: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub triggers: Option<Vec<String>>,
}

// ============================================================================
// List
// ============================================================================

/// List all MCP servers
pub async fn handle_list(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let config = config.read().await;

    // Check if unified_tools is used
    let servers: Vec<McpServerInfo> = if let Some(ref unified) = config.unified_tools {
        unified
            .mcp
            .iter()
            .map(|(name, cfg)| McpServerInfo {
                name: name.clone(),
                command: cfg.command.clone(),
                args: cfg.args.clone(),
                env: cfg.env.clone(),
                enabled: cfg.enabled,
                requires_runtime: cfg.requires_runtime.clone(),
                cwd: cfg.cwd.clone(),
            })
            .collect()
    } else {
        // Fall back to legacy mcp.external_servers
        config
            .mcp
            .external_servers
            .iter()
            .map(|cfg| McpServerInfo {
                name: cfg.name.clone(),
                command: cfg.command.clone(),
                args: cfg.args.clone(),
                env: cfg.env.clone(),
                enabled: true, // Legacy servers don't have enabled field
                requires_runtime: cfg.requires_runtime.clone(),
                cwd: cfg.cwd.clone(),
            })
            .collect()
    };

    JsonRpcResponse::success(request.id, json!({ "servers": servers }))
}

// ============================================================================
// Get
// ============================================================================

/// Parameters for mcp_config.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub name: String,
}

/// Get a single MCP server
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let config = config.read().await;

    // Check unified_tools first
    if let Some(ref unified) = config.unified_tools {
        if let Some(cfg) = unified.mcp.get(&params.name) {
            let info = McpServerInfo {
                name: params.name.clone(),
                command: cfg.command.clone(),
                args: cfg.args.clone(),
                env: cfg.env.clone(),
                enabled: cfg.enabled,
                requires_runtime: cfg.requires_runtime.clone(),
                cwd: cfg.cwd.clone(),
            };
            return JsonRpcResponse::success(request.id, json!({ "server": info }));
        }
    }

    // Fall back to legacy
    if let Some(cfg) = config
        .mcp
        .external_servers
        .iter()
        .find(|s| s.name == params.name)
    {
        let info = McpServerInfo {
            name: cfg.name.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
            enabled: true,
            requires_runtime: cfg.requires_runtime.clone(),
            cwd: cfg.cwd.clone(),
        };
        return JsonRpcResponse::success(request.id, json!({ "server": info }));
    }

    JsonRpcResponse::error(
        request.id,
        INVALID_PARAMS,
        format!("MCP server not found: {}", params.name),
    )
}

// ============================================================================
// Create
// ============================================================================

/// Parameters for mcp_config.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: McpServerConfigJson,
}

/// Create a new MCP server
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Convert JSON config to McpServerConfig
    let server_config = McpServerConfig {
        command: params.config.command.clone(),
        args: params.config.args,
        env: params.config.env,
        cwd: params.config.cwd,
        requires_runtime: params.config.requires_runtime,
        timeout_seconds: params.config.timeout_seconds.unwrap_or(30),
        enabled: params.config.enabled.unwrap_or(true),
        triggers: params.config.triggers,
    };

    // Add server
    {
        let mut cfg = config.write().await;

        // Ensure unified_tools exists
        if cfg.unified_tools.is_none() {
            cfg.unified_tools = Some(crate::config::UnifiedToolsConfig::default());
        }

        if let Some(ref mut unified) = cfg.unified_tools {
            // Check if server already exists
            if unified.mcp.contains_key(&params.name) {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("MCP server already exists: {}", params.name),
                );
            }

            // Insert server
            unified.mcp.insert(params.name.clone(), server_config);
        }

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("mcp".to_string()),
        value: json!({ "action": "created", "server": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "MCP server created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Update
// ============================================================================

/// Parameters for mcp_config.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub name: String,
    pub config: McpServerConfigJson,
}

/// Update an MCP server
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Convert JSON config to McpServerConfig
    let server_config = McpServerConfig {
        command: params.config.command.clone(),
        args: params.config.args,
        env: params.config.env,
        cwd: params.config.cwd,
        requires_runtime: params.config.requires_runtime,
        timeout_seconds: params.config.timeout_seconds.unwrap_or(30),
        enabled: params.config.enabled.unwrap_or(true),
        triggers: params.config.triggers,
    };

    // Update server
    {
        let mut cfg = config.write().await;

        // Ensure unified_tools exists
        if cfg.unified_tools.is_none() {
            cfg.unified_tools = Some(crate::config::UnifiedToolsConfig::default());
        }

        if let Some(ref mut unified) = cfg.unified_tools {
            // Check if server exists
            if !unified.mcp.contains_key(&params.name) {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("MCP server not found: {}", params.name),
                );
            }

            // Update server
            unified.mcp.insert(params.name.clone(), server_config);
        }

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("mcp".to_string()),
        value: json!({ "action": "updated", "server": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "MCP server updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for mcp_config.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

/// Delete an MCP server
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Delete server
    {
        let mut cfg = config.write().await;

        if let Some(ref mut unified) = cfg.unified_tools {
            // Check if server exists
            if !unified.mcp.contains_key(&params.name) {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("MCP server not found: {}", params.name),
                );
            }

            // Remove server
            unified.mcp.remove(&params.name);
        } else {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("MCP server not found: {}", params.name),
            );
        }

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("mcp".to_string()),
        value: json!({ "action": "deleted", "server": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "MCP server deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
