use serde_json::json;

use aleph_protocol::plugins::{PluginListResult, PluginRow, PluginRuntimeStatus};

use super::super::types::{plugin_row, ToggleParams, UninstallParams};
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

/// Read a plugin's stored configuration (`plugin.config.get`).
///
/// A plugin could declare `config_schema` since the manifest types existed and
/// nothing could read or write a value. Returns the manifest's schema
/// alongside the values so a client can render a form without a second call —
/// which is what `config_ui_hints` is for and why it had no consumer outside
/// the authoring-time linter.
pub async fn handle_config_get(request: JsonRpcRequest) -> JsonRpcResponse {
    let Some(name) = request
        .params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
    else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'name'");
    };
    if !is_safe_plugin_name(name) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid plugin name");
    }

    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };
    if let Err(e) = manager.ensure_loaded().await {
        tracing::warn!("Failed to load extensions: {}", e);
    }

    let settings = manager.plugin_settings(name).await;
    let (schema, hints) = {
        let registry = manager.get_plugin_registry().await;
        registry.get_plugin(name).map_or((None, None), |record| {
            match crate::extension::manifest::parse_manifest_from_dir_cached_global(
                &record.root_dir,
            ) {
                Ok(manifest) => (
                    manifest.config_schema.clone(),
                    serde_json::to_value(&manifest.config_ui_hints).ok(),
                ),
                Err(_) => (None, None),
            }
        })
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "name": name,
            "config": settings,
            "schema": schema,
            "ui_hints": hints,
        }),
    )
}

/// Replace a plugin's stored configuration (`plugin.config.set`).
///
/// Validation happens against the plugin's own `config_schema` and reports
/// every violation, not the first. The response says whether a reload is
/// needed rather than reloading implicitly: a reload tears down the plugin's
/// MCP servers and background services, which is not a side effect a config
/// write may smuggle in.
pub async fn handle_config_set(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = request.params.as_ref();
    let Some(name) = params
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'name'");
    };
    if !is_safe_plugin_name(&name) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid plugin name");
    }
    let Some(config) = params.and_then(|p| p.get("config")).cloned() else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'config'");
    };

    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };
    if let Err(e) = manager.ensure_loaded().await {
        tracing::warn!("Failed to load extensions: {}", e);
    }

    match manager.set_plugin_settings(&name, config).await {
        Ok(changed) => JsonRpcResponse::success(
            request.id,
            json!({
                "name": name,
                "changed": changed,
                // A running plugin keeps the configuration it started with.
                "reload_required": changed,
            }),
        ),
        // Schema violations are the caller's to fix, not a server fault —
        // INVALID_PARAMS, so the client is told to change the request rather
        // than to retry it.
        Err(errors) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Configuration rejected:\n  - {}", errors.join("\n  - ")),
        ),
    }
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

    let mut plugins: Vec<PluginRow> = manager
        .get_plugin_info()
        .await
        .into_iter()
        .map(plugin_row)
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
            debug_assert!(
                row.status != PluginRuntimeStatus::Loaded || row.enabled,
                "a `loaded` row must also read enabled — the two answer the \
                 same question and clients pick whichever is handier"
            );
        }
    }

    // Built from the shared contract type, not a `json!` literal: the envelope
    // key is the last part of a wire shape that stays hand-copied, and two
    // functions in the CLI once disagreed about whether it existed at all.
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(PluginListResult { plugins }).unwrap_or_else(|_| json!({})),
    )
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
            crate::extension::plugin_state::forget_plugin_sidecars(&params.name).await;
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
/// Records `enabled = true` in `<data_dir>/plugins.toml` (via
/// `set_plugin_enabled`, the single durable writer) and brings declared
/// autostart services up.
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

    // Persist the preference and sync the live registry, then bring declared
    // autostart services up (no-op for plugins without services; idempotent
    // otherwise). `set_plugin_enabled` owns the durable write — this handler
    // deliberately touches no marker file, because the marker it used to write
    // was never read by anything.
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, true).await;
        manager.sync_plugin_services().await;
    }

    tracing::info!(plugin = %params.name, "Plugin enabled");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Disable a plugin
///
/// Records `enabled = false` in `<data_dir>/plugins.toml` (via
/// `set_plugin_enabled`, the single durable writer) and tears the runtime down.
///
/// The preference survives a restart *and* a `plugin update` — the marker file
/// this replaced could survive neither, and in fact was never read at all.
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

    // Persist the preference and sync the registry, then tear down the runtime
    // — a disabled plugin must not keep background services or transient MCP
    // servers running for the rest of the process lifetime.
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
