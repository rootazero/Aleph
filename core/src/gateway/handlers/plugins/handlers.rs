//! Plugins RPC Handlers
//!
//! Handlers for plugin management: list, install, uninstall, enable, disable.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use once_cell::sync::OnceCell;
use serde_json::json;
use crate::sync_primitives::Arc;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::handlers::parse_params;
use crate::extension::{ContentLoader, ExtensionManager};

use super::types::*;

// ============================================================================
// Global Extension Manager (for plugin tool calls)
// ============================================================================

/// Global extension manager for plugin handlers.
///
/// This is initialized once at gateway startup via `init_extension_manager()`.
/// The OnceCell ensures thread-safe lazy initialization.
static EXTENSION_MANAGER: OnceCell<Arc<ExtensionManager>> = OnceCell::new();

/// Initialize the extension manager for plugin handlers.
///
/// This should be called once during gateway startup, before any
/// `plugins.callTool` requests are processed.
///
/// # Arguments
///
/// * `manager` - The ExtensionManager instance to use for plugin operations
///
/// # Returns
///
/// * `Ok(())` if initialization succeeded
/// * `Err(manager)` if already initialized (returns the passed manager)
pub fn init_extension_manager(
    manager: Arc<ExtensionManager>,
) -> Result<(), Arc<ExtensionManager>> {
    EXTENSION_MANAGER.set(manager)
}

/// Get the extension manager.
///
/// Returns an error response if the manager hasn't been initialized.
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
pub fn get_extension_manager() -> Result<&'static Arc<ExtensionManager>, JsonRpcResponse> {
    EXTENSION_MANAGER.get().ok_or_else(|| {
        JsonRpcResponse::error(
            None,
            INTERNAL_ERROR,
            "Extension manager not initialized. Gateway startup may have failed.".to_string(),
        )
    })
}

/// Check if the extension manager has been initialized.
pub fn is_extension_manager_initialized() -> bool {
    EXTENSION_MANAGER.get().is_some()
}

// ============================================================================
// List
// ============================================================================

/// List all installed plugins
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Ensure plugins are discovered and loaded before listing
    if let Err(e) = manager.ensure_loaded().await {
        tracing::warn!("Failed to load extensions: {}", e);
    }

    let plugins: Vec<PluginInfoJson> = manager
        .get_plugin_info()
        .await
        .into_iter()
        .map(PluginInfoJson::from)
        .collect();

    JsonRpcResponse::success(request.id, json!({ "plugins": plugins }))
}

// ============================================================================
// Install from Git
// ============================================================================

/// Install a plugin from Git repository
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Clone repository and install
    let plugins_dir = crate::extension::default_plugins_dir();

    // Use git2 to clone
    let repo_name = params
        .url
        .split('/')
        .next_back()
        .unwrap_or("plugin")
        .trim_end_matches(".git");
    let dest_path = plugins_dir.join(repo_name);

    if dest_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin already exists at: {}", dest_path.display()),
        );
    }

    match git2::Repository::clone(&params.url, &dest_path) {
        Ok(_) => {
            // Load the installed plugin to get info
            let loader = ContentLoader::new();
            match loader.load_plugin(&dest_path).await {
                Ok(plugin) => {
                    let info = PluginInfoJson::from(plugin.info());
                    JsonRpcResponse::success(request.id, json!({ "plugin": info }))
                }
                Err(e) => {
                    // Cleanup on failure
                    let _ = std::fs::remove_dir_all(&dest_path);
                    JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to load installed plugin: {}", e),
                    )
                }
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clone repository: {}", e),
        ),
    }
}

// ============================================================================
// Install from Zip
// ============================================================================

/// Install plugins from a zip file
pub async fn handle_install_from_zip(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallFromZipParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Decode base64
    let zip_data = match BASE64.decode(&params.data) {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 data: {}", e),
            );
        }
    };

    // Extract and install
    let plugins_dir = crate::extension::default_plugins_dir();
    let temp_path = std::env::temp_dir().join(format!("aleph-plugin-{}.zip", uuid::Uuid::new_v4()));

    // Write temp file
    if let Err(e) = std::fs::write(&temp_path, &zip_data) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to write temp file: {}", e),
        );
    }

    // Extract zip
    let file = match std::fs::File::open(&temp_path) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open zip file: {}", e),
            );
        }
    };

    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read zip archive: {}", e),
            );
        }
    };

    if let Err(e) = archive.extract(&plugins_dir) {
        let _ = std::fs::remove_file(&temp_path);
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to extract zip: {}", e),
        );
    }

    let _ = std::fs::remove_file(&temp_path);

    // Return list of installed plugin names
    // For simplicity, return empty list - caller should use plugins.list to refresh
    JsonRpcResponse::success(request.id, json!({ "installedNames": Vec::<String>::new() }))
}

// ============================================================================
// Uninstall
// ============================================================================

/// Uninstall a plugin
pub async fn handle_uninstall(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UninstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let plugins_dir = crate::extension::default_plugins_dir();
    let plugin_path = plugins_dir.join(&params.name);

    if !plugin_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin not found: {}", params.name),
        );
    }

    match std::fs::remove_dir_all(&plugin_path) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove plugin: {}", e),
        ),
    }
}

// ============================================================================
// Enable/Disable
// ============================================================================

/// Enable a plugin
///
/// Removes the `.disabled` marker file from the plugin directory,
/// allowing the plugin to be discovered and loaded on next scan.
pub async fn handle_enable(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ToggleParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let plugins_dir = crate::extension::default_plugins_dir();
    let plugin_path = plugins_dir.join(&params.name);

    if !plugin_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin not found: {}", params.name),
        );
    }

    let disabled_marker = plugin_path.join(".disabled");
    if disabled_marker.exists() {
        if let Err(e) = std::fs::remove_file(&disabled_marker) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to enable plugin: {}", e),
            );
        }
    }

    tracing::info!(plugin = %params.name, "Plugin enabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Disable a plugin
///
/// Creates a `.disabled` marker file in the plugin directory,
/// preventing the plugin from being discovered and loaded on next scan.
pub async fn handle_disable(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ToggleParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let plugins_dir = crate::extension::default_plugins_dir();
    let plugin_path = plugins_dir.join(&params.name);

    if !plugin_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin not found: {}", params.name),
        );
    }

    let disabled_marker = plugin_path.join(".disabled");
    if !disabled_marker.exists() {
        if let Err(e) = std::fs::write(&disabled_marker, "") {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to disable plugin: {}", e),
            );
        }
    }

    tracing::info!(plugin = %params.name, "Plugin disabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Call Tool
// ============================================================================

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

// ============================================================================
// Execute Command
// ============================================================================

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
        registry.get_command(&params.command_name).map(|cmd| {
            (cmd.plugin_id.clone(), cmd.handler.clone())
        })
    };

    let (registered_plugin_id, handler) = match command_handler {
        Some((pid, h)) => (pid, h),
        None => {
            return JsonRpcResponse::error(
                request.id,
                -32001, // Custom error code for "command not found"
                format!(
                    "Command '{}' not found in registry",
                    params.command_name
                ),
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
        Ok(cmd_result) => {
            JsonRpcResponse::success(request.id, serde_json::to_value(cmd_result).unwrap())
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Command execution failed: {}", e),
        ),
    }
}

// ============================================================================
// Load Plugin
// ============================================================================

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
/// - `kind`: Plugin kind (NodeJs, Wasm, Static)
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

// ============================================================================
// Unload Plugin
// ============================================================================

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_params() {
        let json = json!({"url": "https://github.com/example/plugin.git"});
        let params: InstallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.url, "https://github.com/example/plugin.git");
    }

    #[test]
    fn test_toggle_params() {
        let json = json!({"name": "my-plugin"});
        let params: ToggleParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "my-plugin");
    }

    #[test]
    fn test_call_tool_params() {
        let json = json!({
            "pluginId": "my-plugin",
            "handler": "myTool",
            "args": {"key": "value"}
        });
        let params: CallToolParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "my-plugin");
        assert_eq!(params.handler, "myTool");
        assert_eq!(params.args["key"], "value");
    }

    #[test]
    fn test_call_tool_params_default_args() {
        let json = json!({
            "pluginId": "test",
            "handler": "handler"
        });
        let params: CallToolParams = serde_json::from_value(json).unwrap();
        assert!(params.args.is_null());
    }

    #[test]
    fn test_load_plugin_params() {
        let json = json!({ "path": "/path/to/plugin" });
        let params: LoadPluginParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.path, "/path/to/plugin");
    }

    #[test]
    fn test_unload_plugin_params() {
        let json = json!({ "pluginId": "my-plugin" });
        let params: UnloadPluginParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "my-plugin");
    }

    #[tokio::test]
    async fn test_handle_call_tool_missing_params() {
        let request = JsonRpcRequest::with_id("plugins.callTool", None, json!(1));
        let response = handle_call_tool(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_call_tool_invalid_params() {
        let request = JsonRpcRequest::new(
            "plugins.callTool",
            Some(json!({"invalid": "params"})),
            Some(json!(1)),
        );
        let response = handle_call_tool(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_call_tool_without_manager() {
        // When extension manager is not initialized, should return INTERNAL_ERROR
        // Note: This test only works if extension manager hasn't been initialized
        // in other tests running in the same process.
        if !is_extension_manager_initialized() {
            let request = JsonRpcRequest::new(
                "plugins.callTool",
                Some(json!({
                    "pluginId": "test-plugin",
                    "handler": "testHandler",
                    "args": {}
                })),
                Some(json!(1)),
            );
            let response = handle_call_tool(request).await;

            assert!(response.is_error());
            assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
            assert!(response
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("not initialized"));
        }
    }

    #[tokio::test]
    async fn test_handle_call_tool_with_manager_plugin_not_found() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "plugins.callTool",
            Some(json!({
                "pluginId": "nonexistent-plugin",
                "handler": "testHandler",
                "args": {}
            })),
            Some(json!(1)),
        );
        let response = handle_call_tool(request).await;

        // Should return error because plugin doesn't exist
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn test_handle_load_missing_params() {
        let request = JsonRpcRequest::with_id("plugins.load", None, json!(1));
        let response = handle_load(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_load_invalid_params() {
        let request = JsonRpcRequest::new(
            "plugins.load",
            Some(json!({"invalid": "field"})),
            Some(json!(1)),
        );
        let response = handle_load(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_load_nonexistent_path() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "plugins.load",
            Some(json!({"path": "/nonexistent/path/to/plugin"})),
            Some(json!(1)),
        );
        let response = handle_load(request).await;

        // Should fail because path doesn't exist
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Failed to parse manifest"));
    }

    #[tokio::test]
    async fn test_handle_unload_missing_params() {
        let request = JsonRpcRequest::with_id("plugins.unload", None, json!(1));
        let response = handle_unload(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_unload_invalid_params() {
        let request = JsonRpcRequest::new(
            "plugins.unload",
            Some(json!({"invalid": "field"})),
            Some(json!(1)),
        );
        let response = handle_unload(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_unload_nonexistent_plugin() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "plugins.unload",
            Some(json!({"pluginId": "nonexistent-plugin"})),
            Some(json!(1)),
        );
        let response = handle_unload(request).await;

        // Should fail because plugin is not loaded
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Failed to unload plugin"));
    }

    #[test]
    fn test_execute_command_params() {
        let json = json!({
            "pluginId": "my-plugin",
            "commandName": "status",
            "args": {"verbose": true}
        });
        let params: ExecuteCommandParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "my-plugin");
        assert_eq!(params.command_name, "status");
        assert_eq!(params.args["verbose"], true);
    }

    #[test]
    fn test_execute_command_params_default_args() {
        let json = json!({
            "pluginId": "test-plugin",
            "commandName": "clear"
        });
        let params: ExecuteCommandParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "test-plugin");
        assert_eq!(params.command_name, "clear");
        assert!(params.args.is_null());
    }

    #[tokio::test]
    async fn test_handle_execute_command_missing_params() {
        let request = JsonRpcRequest::with_id("plugins.executeCommand", None, json!(1));
        let response = handle_execute_command(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_execute_command_invalid_params() {
        let request = JsonRpcRequest::new(
            "plugins.executeCommand",
            Some(json!({"invalid": "params"})),
            Some(json!(1)),
        );
        let response = handle_execute_command(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_execute_command_not_found() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "plugins.executeCommand",
            Some(json!({
                "pluginId": "test-plugin",
                "commandName": "nonexistent-command",
                "args": {}
            })),
            Some(json!(1)),
        );
        let response = handle_execute_command(request).await;

        // Should return custom error -32001 because command doesn't exist
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, -32001);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("not found"));
    }
}
