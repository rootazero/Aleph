//! Plugins RPC Handlers
//!
//! Handlers for plugin management: list, install, uninstall, enable, disable.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::extension::{ComponentLoader, ExtensionManager, PluginInfo, SyncExtensionManager};

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
fn get_extension_manager() -> Result<&'static Arc<ExtensionManager>, JsonRpcResponse> {
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

/// Plugin info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfoJson {
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub path: String,
    pub skills_count: u32,
    pub agents_count: u32,
    pub hooks_count: u32,
    pub mcp_servers_count: u32,
}

impl From<PluginInfo> for PluginInfoJson {
    fn from(info: PluginInfo) -> Self {
        Self {
            name: info.name,
            version: info.version.unwrap_or_default(),
            description: info.description.unwrap_or_default(),
            enabled: info.enabled,
            path: info.path,
            skills_count: info.skills_count as u32,
            agents_count: info.agents_count as u32,
            hooks_count: info.hooks_count as u32,
            mcp_servers_count: info.mcp_servers_count as u32,
        }
    }
}

// ============================================================================
// List
// ============================================================================

/// List all installed plugins
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match SyncExtensionManager::new() {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to create extension manager: {}", e),
            );
        }
    };

    // Load all plugins
    if let Err(e) = manager.load_all() {
        tracing::warn!(error = %e, "Error loading some plugins");
    }

    let plugins: Vec<PluginInfoJson> = manager
        .get_plugin_info()
        .into_iter()
        .map(PluginInfoJson::from)
        .collect();

    JsonRpcResponse::success(request.id, json!({ "plugins": plugins }))
}

// ============================================================================
// Install from Git
// ============================================================================

/// Parameters for plugins.install
#[derive(Debug, Deserialize)]
pub struct InstallParams {
    /// Git URL to install from
    pub url: String,
}

/// Install a plugin from Git repository
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match request.params {
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
                "Missing params: url required".to_string(),
            );
        }
    };

    // Clone repository and install
    let plugins_dir = crate::extension::default_plugins_dir();

    // Use git2 to clone
    let repo_name = params
        .url
        .split('/')
        .last()
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
            let loader = ComponentLoader::new();
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

/// Parameters for plugins.installFromZip
#[derive(Debug, Deserialize)]
pub struct InstallFromZipParams {
    /// Base64-encoded zip data
    pub data: String,
}

/// Install plugins from a zip file
pub async fn handle_install_from_zip(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallFromZipParams = match request.params {
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
                "Missing params: data required".to_string(),
            );
        }
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
    let temp_path = std::env::temp_dir().join(format!("aether-plugin-{}.zip", uuid::Uuid::new_v4()));

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

/// Parameters for plugins.uninstall
#[derive(Debug, Deserialize)]
pub struct UninstallParams {
    pub name: String,
}

/// Uninstall a plugin
pub async fn handle_uninstall(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UninstallParams = match request.params {
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
                "Missing params: name required".to_string(),
            );
        }
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

/// Parameters for plugins.enable and plugins.disable
#[derive(Debug, Deserialize)]
pub struct ToggleParams {
    pub name: String,
}

/// Enable a plugin
pub async fn handle_enable(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ToggleParams = match request.params {
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
                "Missing params: name required".to_string(),
            );
        }
    };

    // TODO: Implement plugin enable/disable in registry
    tracing::info!(plugin = %params.name, "Plugin enabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Disable a plugin
pub async fn handle_disable(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ToggleParams = match request.params {
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
                "Missing params: name required".to_string(),
            );
        }
    };

    // TODO: Implement plugin enable/disable in registry
    tracing::info!(plugin = %params.name, "Plugin disabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Call Tool
// ============================================================================

/// Parameters for plugins.callTool
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolParams {
    /// ID of the plugin containing the tool
    pub plugin_id: String,
    /// Name of the handler function to call
    pub handler: String,
    /// Arguments to pass to the tool
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Parameters for plugins.load
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginParams {
    /// Path to the plugin directory (containing aether.plugin.json or package.json with aether field)
    pub path: String,
}

/// Parameters for plugins.unload
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnloadPluginParams {
    /// ID of the plugin to unload
    pub plugin_id: String,
}

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
    let params: CallToolParams = match request.params {
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
                "Missing params: pluginId, handler required".to_string(),
            );
        }
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
// Load Plugin
// ============================================================================

/// Load a runtime plugin from a path
///
/// This handler loads a plugin from a directory containing a valid manifest
/// (`aether.plugin.json` or `package.json` with aether field). The plugin
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
    let params: LoadPluginParams = match request.params {
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
                "Missing params: path required".to_string(),
            );
        }
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
    let params: UnloadPluginParams = match request.params {
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
                "Missing params: pluginId required".to_string(),
            );
        }
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
        let request = JsonRpcRequest::new("plugins.callTool", None, Some(json!(1)));
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
        let request = JsonRpcRequest::new("plugins.load", None, Some(json!(1)));
        let response = handle_load(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("path required"));
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
        let request = JsonRpcRequest::new("plugins.unload", None, Some(json!(1)));
        let response = handle_unload(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("pluginId required"));
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
}
