use serde_json::json;

use aleph_protocol::plugins::{PluginListResult, PluginRow, PluginRuntimeStatus};

use super::super::current_plugin_access;
use super::super::types::{ToggleParams, UninstallParams, plugin_row};
use crate::extension::plugin_target::{PluginTarget, PluginTargetError};
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::get_extension_manager;
use crate::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse};

/// List all installed plugins
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Ensure plugin declarations are available without activating providers.
    if let Err(e) = manager.ensure_discovered().await {
        tracing::warn!("Failed to discover extensions: {}", e);
    }

    let mut plugins: Vec<PluginRow> = manager
        .plugin_info_visible_to(&current_plugin_access())
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

    let manager = match get_extension_manager() {
        Ok(manager) => manager,
        Err(error) => return error.with_id(request.id),
    };
    uninstall_via_registry(
        &manager,
        request.id,
        PluginTarget::from_fields(params.name, params.selector),
    )
    .await
}

pub(crate) async fn uninstall_via_registry(
    manager: &crate::extension::ExtensionManager,
    id: Option<serde_json::Value>,
    target: PluginTarget,
) -> JsonRpcResponse {
    if let Err(error) = manager.ensure_discovered().await {
        return JsonRpcResponse::error(id, INTERNAL_ERROR, error.to_string());
    }
    match manager
        .uninstall_plugin_target(&target, &current_plugin_access())
        .await
    {
        Ok((instance, Ok(()))) => JsonRpcResponse::success(
            id,
            json!({ "ok": true, "pluginId": instance.plugin_id, "selector": instance.selector(), "scope_kind": instance.scope.kind_label() }),
        ),
        Ok((_, Err(error))) => JsonRpcResponse::error(
            id,
            INTERNAL_ERROR,
            format!("Failed to remove plugin: {error}"),
        ),
        Err(error) => plugin_target_error_response(id, error),
    }
}

/// Enable a plugin
///
/// Records `enabled = true` in `<data_dir>/plugins.toml` (via
/// `set_plugin_enabled`, the single durable writer), which mounts the plugin.
pub async fn handle_enable(request: JsonRpcRequest) -> JsonRpcResponse {
    handle_toggle(request, true).await
}

/// Disable a plugin
///
/// Records `enabled = false` in `<data_dir>/plugins.toml` (via
/// `set_plugin_enabled`, the single durable writer) and tears the runtime down.
///
/// The preference survives a restart *and* a `plugin update` — the marker file
/// this replaced could survive neither, and in fact was never read at all.
pub async fn handle_disable(request: JsonRpcRequest) -> JsonRpcResponse {
    handle_toggle(request, false).await
}

async fn handle_toggle(request: JsonRpcRequest, enabled: bool) -> JsonRpcResponse {
    let params: ToggleParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };
    let target = PluginTarget::from_fields(params.name, params.selector);
    set_enabled_via_registry(
        manager,
        request.id,
        &target,
        &current_plugin_access(),
        enabled,
    )
    .await
}

pub(crate) async fn set_enabled_via_registry(
    manager: &crate::extension::ExtensionManager,
    id: Option<serde_json::Value>,
    target: &PluginTarget,
    access: &crate::extension::plugin_target::PluginAccess,
    enabled: bool,
) -> JsonRpcResponse {
    if let Err(error) = manager.ensure_discovered().await {
        tracing::warn!(%error, "Failed to discover extensions");
    }
    match manager
        .set_plugin_target_enabled(target, access, enabled, true)
        .await
    {
        Ok(result) => plugin_control_response(id, result),
        Err(error) => plugin_target_error_response(id, error),
    }
}

pub(crate) fn plugin_target_error_response(
    id: Option<serde_json::Value>,
    error: PluginTargetError,
) -> JsonRpcResponse {
    match error {
        PluginTargetError::Ambiguous { ref selectors } => JsonRpcResponse::error_with_data(
            id,
            INVALID_PARAMS,
            error.to_string(),
            json!({"selectors": selectors}),
        ),
        _ => JsonRpcResponse::error(id, INVALID_PARAMS, error.to_string()),
    }
}

/// Shared thin wire projection for both plugin RPC and the Hub facade.
pub(crate) fn plugin_control_response(
    id: Option<serde_json::Value>,
    result: Result<
        crate::extension::plugin_state::PluginControlOutcome,
        crate::extension::plugin_state::PluginControlError,
    >,
) -> JsonRpcResponse {
    match result {
        Ok(outcome) => JsonRpcResponse::success(id, json!({ "ok": true, "outcome": outcome })),
        Err(error) => JsonRpcResponse::error_with_data(
            id,
            INTERNAL_ERROR,
            error
                .outcome
                .error
                .as_deref()
                .unwrap_or("Plugin control failed"),
            json!({ "ok": false, "outcome": error.outcome }),
        ),
    }
}
