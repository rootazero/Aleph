//! `extensions.toggle` / `extensions.uninstall` — kind-routed lifecycle over
//! the existing MCP / plugin / skill backends.

use crate::domain::skill::SkillId;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::skills::{ensure_shared_system_initialized, shared_system};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
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
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UninstallParams {
    pub id: String,
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
                mgr.set_plugin_enabled(backend, p.enabled).await;
                Ok(())
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
        "plugin" => uninstall_plugin(backend).await,
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

/// A plugin id is joined onto the plugins directory and `remove_dir_all`'d, so
/// it must be a single normal path component — no separators, no `..`, not
/// absolute. Otherwise a crafted `local:plugin:../../x` id could escape the
/// plugins directory. Mirrors `plugins::handlers::manage::is_safe_plugin_name`.
fn is_safe_plugin_id(id: &str) -> bool {
    use std::path::Component;
    let mut comps = std::path::Path::new(id).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// Tear down the plugin runtime then delete its directory, mirroring the
/// existing `plugins.uninstall` handler (stop services before removing files).
async fn uninstall_plugin(plugin_id: &str) -> Result<(), String> {
    if !is_safe_plugin_id(plugin_id) {
        return Err(format!("invalid plugin id: {plugin_id}"));
    }
    let dir = crate::extension::default_plugins_dir().join(plugin_id);
    if !dir.exists() {
        return Err(format!("plugin not found: {plugin_id}"));
    }
    if let Some(mgr) = crate::extension::try_extension_manager() {
        let _ = mgr.unload_runtime_plugin(plugin_id).await;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::extension::plugin_state::forget_plugin_sidecars(plugin_id).await;
    if let Some(mgr) = crate::extension::try_extension_manager() {
        let _ = mgr.reload().await;
    }
    Ok(())
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

    #[test]
    fn plugin_uninstall_id_guard_blocks_traversal() {
        // Regression: a crafted `local:plugin:..` id must never reach remove_dir_all.
        assert!(!is_safe_plugin_id("../../../.ssh"));
        assert!(!is_safe_plugin_id("a/b"));
        assert!(!is_safe_plugin_id("/abs"));
        assert!(!is_safe_plugin_id(".."));
        assert!(!is_safe_plugin_id(""));
        assert!(is_safe_plugin_id("my-plugin"));
    }
}
