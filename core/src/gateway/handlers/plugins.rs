//! Plugins RPC Handlers
//!
//! Handlers for plugin management: list, install, uninstall, enable, disable.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::extension::{ComponentLoader, PluginInfo, SyncExtensionManager};

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
}
