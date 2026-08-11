use serde_json::json;

use super::super::types::{PluginInfoJson, ToggleParams, UninstallParams};
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// A plugin name is joined onto the plugins directory and used for
/// destructive filesystem operations (`remove_dir_all`, marker writes), so it
/// must be a single normal path component — no separators, no `..`, not
/// absolute. Anything else could escape the plugins directory.
fn is_safe_plugin_name(name: &str) -> bool {
    use std::path::Component;
    let mut comps = std::path::Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(_)), None)
    )
}

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

    let mut plugins: Vec<PluginInfoJson> = manager
        .get_plugin_info()
        .await
        .into_iter()
        .map(PluginInfoJson::from)
        .collect();

    // Join the invocation record onto each row. `PluginInfo::name` IS the
    // registry id (`plugin_ops::get_plugin_info` copies `record.id` into it),
    // which is the same key the usage sidecar writes under — so this is an
    // id join, not a name match. `None` for the MCP handle is deliberate: this
    // handler only reads the plugin rows, and asking for an MCP inventory it
    // will not use would cost an actor round-trip per plugin list.
    if !plugins.is_empty() {
        use crate::tools::usage::report::ExtensionKind;
        let report = crate::tools::usage::report::build_report_now(None).await;
        for row in &mut plugins {
            if let Some(entry) = report
                .entries
                .iter()
                .find(|e| e.kind == ExtensionKind::Plugin && e.id == row.name)
            {
                row.usage = Some(aleph_protocol::extension_usage::UsageSummary::from(entry));
            }
        }
    }

    JsonRpcResponse::success(request.id, json!({ "plugins": plugins }))
}

/// Uninstall a plugin
pub async fn handle_uninstall(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: UninstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !is_safe_plugin_name(&params.name) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Invalid plugin name: {}", params.name),
        );
    }

    let plugins_dir = crate::extension::default_plugins_dir();
    let plugin_path = plugins_dir.join(&params.name);

    if !plugin_path.exists() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Plugin not found: {}", params.name),
        );
    }

    // Tear down the runtime BEFORE deleting files: once the plugin dir is
    // gone its service stop handlers can no longer run, which would orphan
    // background services and transient MCP servers for the process lifetime.
    if let Ok(manager) = get_extension_manager() {
        match manager.unload_runtime_plugin(&params.name).await {
            Ok(()) => {}
            // Never loaded into the runtime — nothing to tear down.
            Err(crate::extension::ExtensionError::PluginNotFound(_)) => {}
            Err(e) => tracing::warn!(
                plugin = %params.name,
                error = %e,
                "Failed to unload plugin runtime before uninstall"
            ),
        }
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
            format!("Failed to remove plugin: {e}"),
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

    if !is_safe_plugin_name(&params.name) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Invalid plugin name: {}", params.name),
        );
    }

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
        if let Err(e) = tokio::fs::remove_file(&disabled_marker).await {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to enable plugin: {e}"),
            );
        }
    }

    // Sync with PluginRegistry, then bring declared autostart services up
    // (no-op for plugins without services; idempotent otherwise).
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, true).await;
        manager.sync_plugin_services().await;
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

    if !is_safe_plugin_name(&params.name) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Invalid plugin name: {}", params.name),
        );
    }

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
        if let Err(e) = tokio::fs::write(&disabled_marker, "").await {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to disable plugin: {e}"),
            );
        }
    }

    // Sync with PluginRegistry, then tear down the runtime — a disabled
    // plugin must not keep background services or transient MCP servers
    // running (the `.disabled` marker only affects the next scan).
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, false).await;
        match manager.unload_runtime_plugin(&params.name).await {
            Ok(()) => {}
            // Never loaded into the runtime — nothing to tear down.
            Err(crate::extension::ExtensionError::PluginNotFound(_)) => {}
            Err(e) => tracing::warn!(
                plugin = %params.name,
                error = %e,
                "Failed to unload plugin runtime on disable"
            ),
        }
    }

    tracing::info!(plugin = %params.name, "Plugin disabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
