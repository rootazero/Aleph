//! `extensions.catalog` (cached, filtered, offline-capable; reconciled against
//! the live installed set) and `extensions.installed` (live reconciliation
//! across MCP / plugins / skills).

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::skills::{ensure_shared_system_initialized, shared_system};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::install::mcp_server_id;
use crate::hub::reconcile::{mcp_to_entry, plugin_to_entry, skill_to_entry};
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind};
use crate::mcp::manager::McpManagerHandle;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct CatalogParams {
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    pub query: Option<String>,
}

/// Live-reconciled installed extensions across MCP / plugins / skills.
///
/// Best-effort: a failing or empty backend is logged and skipped — it never
/// aborts, so a flaky MCP actor cannot blank the catalog or installed views.
/// All calls are local (no network), so callers stay offline-capable.
pub async fn collect_installed(mcp: Option<McpManagerHandle>) -> Vec<ExtensionEntry> {
    let mut out = Vec::new();

    if let Some(mcp) = &mcp {
        match mcp.list_servers().await {
            Ok(servers) => out.extend(servers.iter().map(mcp_to_entry)),
            Err(e) => tracing::warn!("collect_installed: mcp list failed: {e}"),
        }
    }

    if let Some(mgr) = crate::extension::try_extension_manager() {
        if let Err(e) = mgr.ensure_loaded().await {
            tracing::warn!("collect_installed: failed to load plugins: {e}");
        }
        out.extend(mgr.list_plugin_records().await.iter().map(plugin_to_entry));
    }

    ensure_shared_system_initialized().await;
    out.extend(
        shared_system()
            .full_status()
            .await
            .iter()
            .map(skill_to_entry),
    );

    out
}

/// Stamp `installed` / `enabled` onto each catalog entry by matching it against
/// the live installed set. MCP matches exactly by its deterministic derived id
/// (`local:mcp:{mcp_server_id(entry.id)}`); Plugin / Skill match by
/// case-insensitive `name` within the same `kind`.
fn mark_installed(catalog: &mut [ExtensionEntry], installed: &[ExtensionEntry]) {
    // (kind.as_str(), lowercased name) -> enabled, for Plugin/Skill matching.
    let by_name: HashMap<(String, String), bool> = installed
        .iter()
        .map(|e| {
            (
                (e.kind.as_str().to_string(), e.name.trim().to_lowercase()),
                e.enabled,
            )
        })
        .collect();

    for e in catalog.iter_mut() {
        let enabled = if e.kind == ExtensionKind::Mcp {
            let expected = format!("local:mcp:{}", mcp_server_id(&e.id));
            installed
                .iter()
                .find(|ie| ie.id == expected)
                .map(|ie| ie.enabled)
        } else {
            by_name
                .get(&(e.kind.as_str().to_string(), e.name.trim().to_lowercase()))
                .copied()
        };
        if let Some(en) = enabled {
            e.installed = true;
            e.enabled = en;
        }
    }
}

/// extensions.catalog — filtered read of the cached catalog, reconciled against
/// the live installed set so browse cards show accurate installed-state.
pub async fn handle_catalog(
    req: JsonRpcRequest,
    cache: Arc<CatalogCache>,
    mcp: Option<McpManagerHandle>,
) -> JsonRpcResponse {
    let p: CatalogParams = if req.params.is_some() {
        match parse_params(&req) {
            Ok(p) => p,
            Err(e) => return e,
        }
    } else {
        CatalogParams::default()
    };
    let filter = CatalogFilter {
        kind: p.kind,
        category: p.category,
        source_id: p.source_id,
        query: p.query,
        ..Default::default()
    };
    match cache.query(&filter).await {
        Ok(mut entries) => {
            let installed = collect_installed(mcp).await;
            mark_installed(&mut entries, &installed);
            let items: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    let mut v = serde_json::to_value(e).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert(
                            "source_label".into(),
                            json!(e.via.clone().unwrap_or_default()),
                        );
                    }
                    v
                })
                .collect();
            JsonRpcResponse::success(req.id, json!({ "extensions": items }))
        }
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// extensions.installed — live reconciled list across all backends.
pub async fn handle_installed(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
) -> JsonRpcResponse {
    let out = collect_installed(mcp).await;
    JsonRpcResponse::success(req.id, json!({ "extensions": out }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};

    fn catalog_entry(id: &str, kind: ExtensionKind, name: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind,
            category: ExtensionCategory::Other,
            name: name.into(),
            description: String::new(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Unverified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: Some("Aleph Hub".into()),
            install_spec: None,
        }
    }

    fn installed_entry(id: &str, kind: ExtensionKind, name: &str, enabled: bool) -> ExtensionEntry {
        let mut e = catalog_entry(id, kind, name);
        e.installed = true;
        e.enabled = enabled;
        e.source_id = "local".into();
        e.via = None;
        e
    }

    #[test]
    fn mcp_entry_marked_installed_by_derived_id() {
        // catalog id "aleph-hub:github" -> install id "aleph-hub_github"
        // -> reconciled installed id "local:mcp:aleph-hub_github"
        let mut catalog = vec![catalog_entry(
            "aleph-hub:github",
            ExtensionKind::Mcp,
            "GitHub",
        )];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    #[test]
    fn mcp_entry_not_installed_when_no_match() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:absent",
            ExtensionKind::Mcp,
            "Nope",
        )];
        let installed = vec![installed_entry(
            "local:mcp:something-else",
            ExtensionKind::Mcp,
            "Other",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }

    #[test]
    fn plugin_entry_marked_installed_by_name_case_insensitive() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:cool-plugin",
            ExtensionKind::Plugin,
            "Cool Plugin",
        )];
        // discovered plugin id differs; matched by name; enabled=false propagates
        let installed = vec![installed_entry(
            "local:plugin:whatever",
            ExtensionKind::Plugin,
            "cool plugin",
            false,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(!catalog[0].enabled);
    }

    #[test]
    fn name_match_does_not_cross_kinds() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:x",
            ExtensionKind::Skill,
            "Shared Name",
        )];
        let installed = vec![installed_entry(
            "local:plugin:x",
            ExtensionKind::Plugin,
            "Shared Name",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }

    #[test]
    fn official_primer_slug_reconciles_against_live_server() {
        // primer id "aleph-hub:volcengine-veimagex" -> server id "aleph-hub_volcengine-veimagex"
        let mut catalog = vec![catalog_entry(
            "aleph-hub:volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
        )];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
    }

    #[test]
    fn skill_entry_marked_installed_by_name_case_insensitive() {
        // The primer's "aleph-hub:pdf-tools" Skill entry collapses against a live
        // local:skill entry of the same name — this is why official skills show
        // installed with NO reconcile change (the convergence's load-bearing fact).
        let mut catalog = vec![catalog_entry(
            "aleph-hub:pdf-tools",
            ExtensionKind::Skill,
            "PDF Tools",
        )];
        let installed = vec![installed_entry(
            "local:skill:pdf-tools",
            ExtensionKind::Skill,
            "pdf tools",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    #[test]
    fn skill_entry_not_installed_when_name_differs() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:pdf-tools",
            ExtensionKind::Skill,
            "PDF Tools",
        )];
        let installed = vec![installed_entry(
            "local:skill:other",
            ExtensionKind::Skill,
            "Other Skill",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }
}
