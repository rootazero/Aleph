//! MCP Server RPC Handlers
//!
//! Handlers for MCP server management: list, add, update, delete, status, logs,
//! start, stop, restart, and capability aggregation (tools, resources, prompts).
//!
//! These handlers are wired to the McpManagerHandle actor for server lifecycle
//! management and capability discovery.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND};
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle};

// ============================================================================
// Param Types
// ============================================================================

/// Parameters for mcp.add
#[derive(Debug, Deserialize)]
pub struct AddParams {
    pub config: McpManagerConfig,
}

/// Parameters for mcp.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub config: McpManagerConfig,
}

/// Parameters for server ID-based operations
#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

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

// ============================================================================
// Helper: Parse params
// ============================================================================

fn parse_params<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
    required_field: &str,
) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            format!("Missing params: {} required", required_field),
        )),
    }
}

// ============================================================================
// List
// ============================================================================

/// List all MCP servers
pub async fn handle_list(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    match handle.list_servers().await {
        Ok(servers) => JsonRpcResponse::success(request.id, json!({ "servers": servers })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Add
// ============================================================================

/// Add a new MCP server
pub async fn handle_add(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: AddParams = match parse_params(&request, "config") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let server_id = params.config.id.clone();

    match handle.add_server(params.config).await {
        Ok(()) => {
            tracing::info!(id = %server_id, "MCP server added");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Update
// ============================================================================

/// Update an MCP server configuration
///
/// This uses upsert semantics - removes the old server (if exists) and adds the new one.
pub async fn handle_update(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request, "config") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let server_id = params.config.id.clone();

    // Remove old server (ignore if not found)
    let _ = handle.remove_server(&server_id).await;

    // Add updated server
    match handle.add_server(params.config).await {
        Ok(()) => {
            tracing::info!(id = %server_id, "MCP server updated");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Delete
// ============================================================================

/// Delete an MCP server
pub async fn handle_delete(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.remove_server(&params.id).await {
        Ok(()) => {
            tracing::info!(id = %params.id, "MCP server deleted");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Status
// ============================================================================

/// Get MCP server detailed status
pub async fn handle_status(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.get_status(&params.id).await {
        Ok(Some(status)) => JsonRpcResponse::success(request.id, json!(status)),
        Ok(None) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("Server not found: {}", params.id),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Logs
// ============================================================================

/// Get MCP server logs
///
/// Note: Logging is not yet implemented in the actor. Returns empty logs for now.
pub async fn handle_logs(request: JsonRpcRequest, _handle: McpManagerHandle) -> JsonRpcResponse {
    let _params: LogsParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // TODO: Implement log retrieval when logging is added to the actor
    JsonRpcResponse::success(request.id, json!({ "logs": Vec::<String>::new() }))
}

// ============================================================================
// Start
// ============================================================================

/// Start a stopped MCP server
pub async fn handle_start(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.start_server(&params.id).await {
        Ok(()) => {
            tracing::info!(id = %params.id, "MCP server started");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Stop
// ============================================================================

/// Stop a running MCP server
pub async fn handle_stop(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.stop_server(&params.id).await {
        Ok(()) => {
            tracing::info!(id = %params.id, "MCP server stopped");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Restart
// ============================================================================

/// Restart an MCP server
pub async fn handle_restart(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request, "id") {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.restart_server(&params.id).await {
        Ok(()) => {
            tracing::info!(id = %params.id, "MCP server restarted");
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// List Tools (Aggregated)
// ============================================================================

/// List all tools from all healthy MCP servers
pub async fn handle_list_tools(
    request: JsonRpcRequest,
    handle: McpManagerHandle,
) -> JsonRpcResponse {
    match handle.aggregate_tools().await {
        Ok(tools) => JsonRpcResponse::success(request.id, json!({ "tools": tools })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// List Resources (Aggregated)
// ============================================================================

/// List all resources from all healthy MCP servers
pub async fn handle_list_resources(
    request: JsonRpcRequest,
    handle: McpManagerHandle,
) -> JsonRpcResponse {
    match handle.aggregate_resources().await {
        Ok(resources) => JsonRpcResponse::success(request.id, json!({ "resources": resources })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// List Prompts (Aggregated)
// ============================================================================

/// List all prompts from all healthy MCP servers
pub async fn handle_list_prompts(
    request: JsonRpcRequest,
    handle: McpManagerHandle,
) -> JsonRpcResponse {
    match handle.aggregate_prompts().await {
        Ok(prompts) => JsonRpcResponse::success(request.id, json!({ "prompts": prompts })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Placeholder handlers (for registration before McpManagerHandle is available)
// ============================================================================

/// Placeholder for mcp.list when handle is not available
pub async fn handle_list_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.add when handle is not available
pub async fn handle_add_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.update when handle is not available
pub async fn handle_update_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.delete when handle is not available
pub async fn handle_delete_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.status when handle is not available
pub async fn handle_status_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.logs when handle is not available
pub async fn handle_logs_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.start when handle is not available
pub async fn handle_start_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.stop when handle is not available
pub async fn handle_stop_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.restart when handle is not available
pub async fn handle_restart_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.tools when handle is not available
pub async fn handle_list_tools_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.resources when handle is not available
pub async fn handle_list_resources_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

/// Placeholder for mcp.prompts when handle is not available
pub async fn handle_list_prompts_placeholder(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "MCP Manager not initialized".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_add_params_deserialize() {
        let json = json!({
            "config": {
                "id": "test-server",
                "name": "Test Server",
                "transport": "stdio",
                "command": "/usr/bin/test"
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "test-server");
        assert_eq!(params.config.name, "Test Server");
        assert_eq!(params.config.command, Some("/usr/bin/test".to_string()));
    }

    #[test]
    fn test_add_params_with_args() {
        let json = json!({
            "config": {
                "id": "node-server",
                "name": "Node Server",
                "command": "npx",
                "args": ["@modelcontextprotocol/server-filesystem"],
                "requires_runtime": "node"
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "node-server");
        assert_eq!(params.config.command, Some("npx".to_string()));
        assert_eq!(params.config.args, vec!["@modelcontextprotocol/server-filesystem"]);
        assert_eq!(params.config.requires_runtime, Some("node".to_string()));
    }

    #[test]
    fn test_id_params_deserialize() {
        let json = json!({"id": "test-server"});
        let params: IdParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "test-server");
    }

    #[test]
    fn test_logs_params_defaults() {
        let json = json!({"id": "test-server"});
        let params: LogsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "test-server");
        assert_eq!(params.max_lines, 100);
    }

    #[test]
    fn test_logs_params_custom_max_lines() {
        let json = json!({"id": "test-server", "max_lines": 500});
        let params: LogsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.max_lines, 500);
    }

    #[test]
    fn test_update_params_deserialize() {
        let json = json!({
            "config": {
                "id": "test-server",
                "name": "Updated Server",
                "command": "/usr/bin/new-cmd"
            }
        });
        let params: UpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "test-server");
        assert_eq!(params.config.name, "Updated Server");
    }

    #[test]
    fn test_add_params_http_transport() {
        let json = json!({
            "config": {
                "id": "remote-server",
                "name": "Remote Server",
                "transport": "http",
                "url": "https://api.example.com/mcp"
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "remote-server");
        assert_eq!(params.config.url, Some("https://api.example.com/mcp".to_string()));
    }

    #[test]
    fn test_add_params_with_env() {
        let json = json!({
            "config": {
                "id": "env-server",
                "name": "Server with Env",
                "command": "/usr/bin/server",
                "env": {
                    "API_KEY": "secret",
                    "DEBUG": "true"
                }
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.env.len(), 2);
        assert_eq!(params.config.env.get("API_KEY"), Some(&"secret".to_string()));
    }
}
