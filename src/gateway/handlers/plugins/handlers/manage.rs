use serde_json::json;

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::types::*;

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
        Ok(()) => {
            if let Ok(manager) = get_extension_manager() {
                if let Err(e) = manager.reload().await {
                    tracing::warn!("Failed to refresh extensions after uninstall: {}", e);
                }
            }
            JsonRpcResponse::success(request.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove plugin: {}", e),
        ),
    }
}

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

    // Sync with PluginRegistry
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, true).await;
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

    // Sync with PluginRegistry
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, false).await;
    }

    tracing::info!(plugin = %params.name, "Plugin disabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
