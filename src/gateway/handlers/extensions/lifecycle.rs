//! `extensions.toggle` / `extensions.uninstall` — kind-routed lifecycle over
//! the existing MCP / plugin / skill backends.

use crate::domain::skill::SkillId;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::skills::{ensure_shared_system_initialized, shared_system};
use crate::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS, JsonRpcRequest, JsonRpcResponse};
use crate::hub::cache::CatalogCache;
use crate::hub::types::ExtensionKind;
use crate::mcp::manager::McpManagerHandle;
use crate::skill::SkillConfigUpdate;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ToggleParams {
    pub id: String,
    #[serde(default)]
    pub selector: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UninstallParams {
    pub id: String,
    #[serde(default)]
    pub selector: Option<String>,
}

/// Parse a façade id of the form `local:{kind}:{backend_id}`.
///
/// Returns `(kind, backend_id)`; `None` for ids that are not installed-local
/// (e.g. catalog ids like `mcp-official:io.x/y`). `split_once` stops at the
/// first `':'`, so a backend id may itself contain `':'`.
pub fn parse_local_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("local:")?;
    rest.split_once(':')
}

fn plugin_target(
    backend: &str,
    selector: Option<String>,
) -> crate::extension::plugin_target::PluginTarget {
    let selector = selector.or_else(|| {
        (backend.starts_with("g:") || backend.starts_with("p:")).then(|| backend.to_owned())
    });
    crate::extension::plugin_target::PluginTarget::from_fields(backend.to_owned(), selector)
}

/// extensions.toggle — enable/disable an installed extension, routed by kind.
pub async fn handle_toggle(req: JsonRpcRequest, mcp: Option<McpManagerHandle>) -> JsonRpcResponse {
    let p: ToggleParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some((kind, backend)) = parse_local_id(&p.id) else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "toggle requires an installed (local:) id",
        );
    };
    if kind == "skill" {
        ensure_shared_system_initialized().await;
    }
    let result: Result<(), String> = match kind {
        "mcp" => match mcp {
            Some(mcp) => {
                let r = if p.enabled {
                    mcp.start_server(backend).await
                } else {
                    mcp.stop_server(backend).await
                };
                r.map_err(|e| e.to_string())
            }
            None => Err("mcp manager unavailable".to_string()),
        },
        "plugin" => match crate::extension::try_extension_manager() {
            Some(mgr) => {
                return toggle_plugin(
                    mgr,
                    req.id,
                    backend,
                    p.selector,
                    &crate::gateway::handlers::plugins::current_plugin_access(),
                    p.enabled,
                )
                .await;
            }
            None => Err("extension manager unavailable".to_string()),
        },
        "skill" => shared_system()
            .update_config(
                &SkillId::new(backend),
                SkillConfigUpdate::SetEnabled(p.enabled),
            )
            .await
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown kind: {other}")),
    };
    match result {
        Ok(()) => JsonRpcResponse::success(req.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    }
}

/// Plugin arm with an explicit manager for isolated caller-propagation tests.
/// Keep the wire projection shared with the plugin RPC; no Hub-owned state.
pub(crate) async fn toggle_plugin(
    manager: &crate::extension::ExtensionManager,
    id: Option<serde_json::Value>,
    backend: &str,
    selector: Option<String>,
    access: &crate::extension::plugin_target::PluginAccess,
    enabled: bool,
) -> JsonRpcResponse {
    let target = plugin_target(backend, selector);
    crate::gateway::handlers::plugins::set_enabled_via_registry(
        manager, id, &target, access, enabled,
    )
    .await
}

/// extensions.uninstall — remove an installed extension, routed by kind.
pub async fn handle_uninstall(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
) -> JsonRpcResponse {
    let p: UninstallParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some((kind, backend)) = parse_local_id(&p.id) else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "uninstall requires an installed (local:) id",
        );
    };
    if kind == "skill" {
        ensure_shared_system_initialized().await;
    }
    let result: Result<(), String> = match kind {
        "mcp" => match mcp {
            Some(mcp) => mcp.remove_server(backend).await.map_err(|e| e.to_string()),
            None => Err("mcp manager unavailable".to_string()),
        },
        "plugin" => {
            let Some(manager) = crate::extension::try_extension_manager() else {
                return JsonRpcResponse::error(
                    req.id,
                    INTERNAL_ERROR,
                    "extension manager unavailable",
                );
            };
            let target = plugin_target(backend, p.selector);
            let response =
                crate::gateway::handlers::plugins::uninstall_via_registry(manager, req.id, target)
                    .await;
            if let Some(result) = &response.result {
                if let Some(key) = result["pluginId"]
                    .as_str()
                    .filter(|_| result["scope_kind"] == "global")
                {
                    if let Err(error) = cache
                        .forget_installed_origin(ExtensionKind::Plugin, key)
                        .await
                    {
                        tracing::warn!(id = %p.id, %error, "failed to clear install origin");
                    }
                }
            }
            return response;
        }
        "skill" => shared_system()
            .remove_skill(&SkillId::new(backend))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown kind: {other}")),
    };
    match result {
        Ok(()) => {
            // Drop the provenance row so a later hand-rolled reinstall of the
            // same name cannot inherit the removed copy's version and light a
            // false update badge. Best-effort: never fails a completed removal.
            if let Some(k) = origin_kind(kind) {
                if let Err(e) = cache.forget_installed_origin(k, backend).await {
                    tracing::warn!(id = %p.id, error = %e, "failed to clear install origin");
                }
            }
            JsonRpcResponse::success(req.id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    }
}

/// Map a façade id's `kind` segment onto the ledger's `ExtensionKind`.
fn origin_kind(kind: &str) -> Option<ExtensionKind> {
    match kind {
        "mcp" => Some(ExtensionKind::Mcp),
        "plugin" => Some(ExtensionKind::Plugin),
        "skill" => Some(ExtensionKind::Skill),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_mcp_id() {
        assert_eq!(parse_local_id("local:mcp:github"), Some(("mcp", "github")));
    }

    #[test]
    fn rejects_non_local_id() {
        assert_eq!(parse_local_id("mcp-official:io.x/y"), None);
    }

    #[test]
    fn handles_backend_ids_with_colons() {
        // split_once stops at the first ':', so a backend id may itself contain ':'.
        assert_eq!(
            parse_local_id("local:skill:my:skill"),
            Some(("skill", "my:skill"))
        );
    }
}
