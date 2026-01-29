//! MCP Server RPC Handlers
//!
//! Handlers for MCP server management: list, add, update, delete, status, logs.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::mcp::McpServerConfig;

/// MCP server info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct McpServerInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// MCP server status
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub status: String, // "running", "stopped", "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// List
// ============================================================================

/// List all MCP servers
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Get from MCP manager state
    // For now, return empty list
    JsonRpcResponse::success(request.id, json!({ "servers": Vec::<McpServerInfo>::new() }))
}

// ============================================================================
// Add
// ============================================================================

/// Parameters for mcp.add
#[derive(Debug, Deserialize)]
pub struct AddParams {
    pub config: McpServerConfigJson,
}

/// MCP server config from JSON
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfigJson {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Add a new MCP server
pub async fn handle_add(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: AddParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: config required".to_string(),
            );
        }
    };

    // TODO: Add to MCP manager
    tracing::info!(id = %params.config.id, "MCP server added");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Update
// ============================================================================

/// Parameters for mcp.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub config: McpServerConfigJson,
}

/// Update an MCP server configuration
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UpdateParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: config required".to_string(),
            );
        }
    };

    // TODO: Update in MCP manager
    tracing::info!(id = %params.config.id, "MCP server updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for mcp.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub id: String,
}

/// Delete an MCP server
pub async fn handle_delete(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: DeleteParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    // TODO: Delete from MCP manager
    tracing::info!(id = %params.id, "MCP server deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Status
// ============================================================================

/// Parameters for mcp.status
#[derive(Debug, Deserialize)]
pub struct StatusParams {
    pub id: String,
}

/// Get MCP server status
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: StatusParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    // TODO: Get from MCP manager
    JsonRpcResponse::success(
        request.id,
        json!(McpServerStatus {
            id: params.id,
            status: "stopped".to_string(),
            error: None,
        }),
    )
}

// ============================================================================
// Logs
// ============================================================================

/// Parameters for mcp.logs
#[derive(Debug, Deserialize)]
pub struct LogsParams {
    pub id: String,
    #[serde(default = "default_max_lines")]
    pub max_lines: u32,
}

fn default_max_lines() -> u32 {
    100
}

/// Get MCP server logs
pub async fn handle_logs(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LogsParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    // TODO: Get from MCP manager
    JsonRpcResponse::success(request.id, json!({ "logs": Vec::<String>::new() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_params() {
        let json = json!({
            "config": {
                "id": "test-server",
                "name": "Test Server",
                "command": "node",
                "args": ["server.js"]
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "test-server");
        assert_eq!(params.config.command, "node");
    }

    #[test]
    fn test_logs_params_defaults() {
        let json = json!({"id": "test-server"});
        let params: LogsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.max_lines, 100);
    }
}
