use serde_json::json;

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::types::*;

/// Call a tool on a loaded runtime plugin
///
/// This handler invokes a tool handler registered by a Node.js or WASM plugin.
/// The plugin must be loaded first via `plugins.load`.
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
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Tool call failed: {}", e),
        ),
    }
}

/// Execute a direct command registered by a plugin
///
/// This handler executes a direct command (e.g., `/status`, `/clear`) that was
/// registered by a runtime plugin. Direct commands execute immediately without
/// LLM involvement and return a result to display to the user.
///
/// # Params
/// - `pluginId`: ID of the plugin that registered the command
/// - `commandName`: Name of the command to execute (without leading slash)
/// - `args`: JSON arguments to pass to the command handler
///
/// # Returns
/// - `result`: The command's DirectCommandResult containing content, data, and success flag
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or command execution failed
/// - `INVALID_PARAMS`: Missing or invalid parameters
/// - `-32001`: Command not found in plugin
pub async fn handle_execute_command(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ExecuteCommandParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Look up the command in the plugin registry
    let command_handler = {
        let registry = manager.get_plugin_registry().await;
        registry
            .get_command(&params.command_name)
            .map(|cmd| (cmd.plugin_id.clone(), cmd.handler.clone()))
    };

    let (registered_plugin_id, handler) = match command_handler {
        Some((pid, h)) => (pid, h),
        None => {
            return JsonRpcResponse::error(
                request.id,
                -32001, // Custom error code for "command not found"
                format!("Command '{}' not found in registry", params.command_name),
            );
        }
    };

    // Validate that the command belongs to the specified plugin
    if registered_plugin_id != params.plugin_id {
        return JsonRpcResponse::error(
            request.id,
            -32001,
            format!(
                "Command '{}' belongs to plugin '{}', not '{}'",
                params.command_name, registered_plugin_id, params.plugin_id
            ),
        );
    }

    // Execute the command via the extension manager
    match manager
        .execute_plugin_command(&params.plugin_id, &handler, params.args)
        .await
    {
        Ok(cmd_result) => match serde_json::to_value(cmd_result) {
            Ok(v) => JsonRpcResponse::success(request.id, v),
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to serialize command result: {}", e),
            ),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Command execution failed: {}", e),
        ),
    }
}

/// Load a runtime plugin from a path
///
/// This handler loads a plugin from a directory containing a valid manifest
/// (`aleph.plugin.json` or `package.json` with aleph field). The plugin
/// is loaded into the appropriate runtime (Node.js or WASM) based on its kind.
///
/// # Params
/// - `path`: Path to the plugin directory
///
/// # Returns
/// - `pluginId`: ID of the loaded plugin
/// - `name`: Human-readable name
/// - `kind`: Plugin kind (Mcp, Wasm, Static)
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or loading failed
/// - `INVALID_PARAMS`: Missing path or invalid manifest
pub async fn handle_load(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LoadPluginParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Parse manifest from path
    let path = std::path::Path::new(&params.path);
    let manifest = match crate::extension::manifest::parse_manifest_from_dir(path).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Failed to parse manifest: {}", e),
            );
        }
    };

    // Load plugin into runtime
    if let Err(e) = manager.load_runtime_plugin(&manifest).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to load plugin: {}", e),
        );
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "pluginId": manifest.id,
            "name": manifest.name,
            "kind": format!("{:?}", manifest.kind),
        }),
    )
}

/// Unload a runtime plugin
///
/// This handler unloads a previously loaded plugin from its runtime.
/// The plugin is removed from the loader's tracking but tools/hooks
/// may still be registered in the registry.
///
/// # Params
/// - `pluginId`: ID of the plugin to unload
///
/// # Returns
/// - `ok`: true if successful
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or plugin not found
/// - `INVALID_PARAMS`: Missing pluginId
pub async fn handle_unload(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UnloadPluginParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Unload from runtime
    match manager.unload_runtime_plugin(&params.plugin_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to unload plugin: {}", e),
        ),
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
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({ "ok": true, "pluginId": params.plugin_id }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reload plugin: {}", e),
        ),
    }
}
