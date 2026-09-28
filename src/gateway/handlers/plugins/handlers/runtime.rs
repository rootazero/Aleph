use serde_json::json;

use super::super::types::{CallToolParams, ReloadPluginParams};
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Call a tool on a loaded runtime plugin
///
/// This handler invokes a tool handler registered by a Node.js or WASM plugin.
/// The plugin must be mounted (enabled) — WASM modules load at mount.
///
/// # Params
/// - `pluginId`: Plugin that provides the tool
/// - `handler`: Handler function name
/// - `args`: JSON arguments to pass to the tool
///
/// # Returns
/// - `result`: The tool's return value
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or tool call failed
/// - `INVALID_PARAMS`: Missing or invalid parameters
pub async fn handle_call_tool(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: CallToolParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Call the plugin tool
    match manager
        .call_plugin_tool(&params.plugin_id, &params.handler, params.args)
        .await
    {
        Ok(result) => JsonRpcResponse::success(request.id, json!({ "result": result })),
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Tool call failed: {e}"))
        }
    }
}

/// Hot-reload a plugin by ID.
///
/// Unregisters all existing capabilities, re-parses the manifest from disk,
/// and re-registers updated capabilities. Useful for development and live
/// updates without restarting the server.
///
/// # Params
/// - `pluginId`: ID of the plugin to reload
///
/// # Returns
/// - `ok`: true if successful
/// - `pluginId`: the reloaded plugin's ID
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized, plugin not found, or reload failed
/// - `INVALID_PARAMS`: Missing pluginId
pub async fn handle_reload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ReloadPluginParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    match manager.reload_plugin(&params.plugin_id).await {
        Ok(status) => JsonRpcResponse::success(
            request.id,
            json!({ "ok": true, "pluginId": params.plugin_id, "status": status.label() }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reload plugin: {e}"),
        ),
    }
}
