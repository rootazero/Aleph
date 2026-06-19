//! MCP Configuration RPC Handlers
//!
//! Handlers for MCP server configuration management: list, create, update, delete.
//! These handlers manage MCP server configurations in the config file.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
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
// Secret env redaction (stable-echo at the trust boundary)
// ============================================================================

/// Env var names matching these substrings (case-insensitive) are treated as
/// secrets: their values are redacted on read and preserved on blank update.
/// Mirrors the Panel's `is_secret_env_key` so masking is consistent both ways.
fn is_secret_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "SECRET", "TOKEN", "PASSWORD", "PASS", "CREDENTIAL"]
        .iter()
        .any(|needle| upper.contains(needle))
}

/// Redact secret env values before sending to the Panel: secret-looking keys
/// keep their name but their value is blanked, so the stored secret never leaves
/// the host. Non-secret values (e.g. `DOMAIN`, `SERVICE_ID`) pass through.
fn redact_secret_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    env.iter()
        .map(|(k, v)| {
            if is_secret_env_key(k) {
                (k.clone(), String::new())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

/// Merge an incoming env (from the Panel) with the stored env: a secret-looking
/// key whose incoming value is blank keeps the previously stored value (stable
/// echo — blank means "unchanged"). A non-blank value replaces it (rotation),
/// and non-secret keys pass through as sent.
fn merge_secret_env(
    incoming: HashMap<String, String>,
    existing: &HashMap<String, String>,
) -> HashMap<String, String> {
    incoming
        .into_iter()
        .map(|(k, v)| {
            if v.is_empty() && is_secret_env_key(&k) {
                if let Some(prev) = existing.get(&k) {
                    return (k, prev.clone());
                }
            }
            (k, v)
        })
        .collect()
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
                env: redact_secret_env(&cfg.env),
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
                env: redact_secret_env(&cfg.env),
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

/// Parameters for `mcp_config.get`
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
                env: redact_secret_env(&cfg.env),
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

/// Parameters for `mcp_config.create`
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
        if let Err(e) = cfg.save_incremental(&["unified_tools"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

/// Parameters for `mcp_config.update`
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

    // Convert JSON config to McpServerConfig. The env is merged with the stored
    // env inside the lock so blanked secrets keep their previous value.
    let incoming_env = params.config.env;
    let mut server_config = McpServerConfig {
        command: params.config.command.clone(),
        args: params.config.args,
        env: HashMap::new(),
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
            // Check if server exists, capturing its stored env for stable-echo merge.
            let existing_env = match unified.mcp.get(&params.name) {
                Some(existing) => existing.env.clone(),
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("MCP server not found: {}", params.name),
                    );
                }
            };

            // Blank secret values keep the stored secret; new values rotate it.
            server_config.env = merge_secret_env(incoming_env, &existing_env);

            // Update server
            unified.mcp.insert(params.name.clone(), server_config);
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["unified_tools"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

/// Parameters for `mcp_config.delete`
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
        if let Err(e) = cfg.save_incremental(&["unified_tools"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn secret_keys_detected_case_insensitively() {
        assert!(is_secret_env_key("VOLCENGINE_ACCESS_KEY"));
        assert!(is_secret_env_key("VOLCENGINE_SECRET_KEY"));
        assert!(is_secret_env_key("api_token"));
        assert!(is_secret_env_key("DB_PASSWORD"));
        assert!(!is_secret_env_key("SERVICE_ID"));
        assert!(!is_secret_env_key("DOMAIN"));
    }

    #[test]
    fn redact_blanks_secrets_keeps_keys_and_nonsecrets() {
        let input = env(&[
            ("VOLCENGINE_ACCESS_KEY", "ak-real"),
            ("SECRET_KEY", "sk-real"),
            ("SERVICE_ID", "svc-123"),
            ("DOMAIN", "img.example.com"),
        ]);
        let out = redact_secret_env(&input);
        assert_eq!(out.get("VOLCENGINE_ACCESS_KEY"), Some(&String::new()));
        assert_eq!(out.get("SECRET_KEY"), Some(&String::new()));
        assert_eq!(out.get("SERVICE_ID"), Some(&"svc-123".to_string()));
        assert_eq!(out.get("DOMAIN"), Some(&"img.example.com".to_string()));
        // Keys are preserved so the Panel still shows the var is configured.
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn merge_blank_secret_keeps_stored_value() {
        let existing = env(&[("ACCESS_KEY", "ak-stored"), ("SERVICE_ID", "old")]);
        // Panel echoes back a blanked secret + an edited non-secret.
        let incoming = env(&[("ACCESS_KEY", ""), ("SERVICE_ID", "new")]);
        let merged = merge_secret_env(incoming, &existing);
        assert_eq!(merged.get("ACCESS_KEY"), Some(&"ak-stored".to_string()));
        assert_eq!(merged.get("SERVICE_ID"), Some(&"new".to_string()));
    }

    #[test]
    fn merge_nonblank_secret_rotates() {
        let existing = env(&[("ACCESS_KEY", "ak-old")]);
        let incoming = env(&[("ACCESS_KEY", "ak-new")]);
        let merged = merge_secret_env(incoming, &existing);
        assert_eq!(merged.get("ACCESS_KEY"), Some(&"ak-new".to_string()));
    }

    #[test]
    fn merge_blank_secret_without_existing_stays_blank() {
        let existing = env(&[]);
        let incoming = env(&[("NEW_TOKEN", "")]);
        let merged = merge_secret_env(incoming, &existing);
        assert_eq!(merged.get("NEW_TOKEN"), Some(&String::new()));
    }
}
