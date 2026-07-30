//! `extensions.catalog` (cached, filtered, offline-capable; reconciled against
//! the live installed set) and `extensions.installed` (live reconciliation
//! across MCP / plugins / skills).

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::reconcile::{collect_installed, mark_installed_state};
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
            // Best-effort, like every other backend read here: a ledger read
            // failure costs the update badge, never the catalog.
            let origins = cache.origins().await.unwrap_or_else(|e| {
                tracing::warn!("handle_catalog: install origin read failed: {e}");
                Vec::new()
            });
            mark_installed_state(&mut entries, &installed, &origins);
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

/// Stamp `update_available` onto the *installed* list.
///
/// The installed panel is where the update badge actually renders, so the ledger
/// has to reach this path too — computing it only for browse cards would leave
/// the badge dark forever. Walks façade id → ledger row → catalog entry:
/// `local:{kind}:{backend}` identifies the backend object,
/// `origin::local_ref_addresses` finds the row that installed it, and the row's
/// `entry_id` names the catalog entry to compare against.
async fn stamp_updates_from_ledger(installed: &mut [ExtensionEntry], cache: &CatalogCache) {
    let origins = match cache.origins().await {
        Ok(o) if !o.is_empty() => o,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("handle_installed: install origin read failed: {e}");
            return;
        }
    };
    let catalog: HashMap<String, ExtensionEntry> =
        match cache.query(&CatalogFilter::default()).await {
            Ok(rows) => rows.into_iter().map(|e| (e.id.clone(), e)).collect(),
            Err(e) => {
                tracing::warn!("handle_installed: catalog read failed: {e}");
                return;
            }
        };
    for e in installed.iter_mut() {
        let Some((kind, backend)) =
            crate::gateway::handlers::extensions::lifecycle::parse_local_id(&e.id)
        else {
            continue;
        };
        let Some(origin) = origins.iter().find(|o| {
            o.kind.as_str() == kind
                && crate::hub::origin::local_ref_addresses(&o.local_ref, backend)
        }) else {
            continue;
        };
        if let Some(offered) = catalog.get(&origin.entry_id) {
            e.update_available = crate::hub::origin::update_available(origin, offered);
        }
    }
}

/// extensions.installed — live reconciled list across all backends, with the
/// update badge stamped from the install provenance ledger.
pub async fn handle_installed(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
) -> JsonRpcResponse {
    let mut out = collect_installed(mcp).await;
    stamp_updates_from_ledger(&mut out, &cache).await;
    JsonRpcResponse::success(req.id, json!({ "extensions": out }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};

    // Reconciliation itself is tested in `hub::reconcile`; these cover only the
    // façade-id → ledger → catalog walk that this handler owns.
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

    async fn seeded_cache(offered_version: &str, installed_version: &str) -> CatalogCache {
        let cache = CatalogCache::open_in_memory().unwrap();
        let spec = crate::hub::types::InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["@gh/mcp".into()],
            env: vec![],
        };
        let mut offered = catalog_entry("aleph-hub:github", ExtensionKind::Mcp, "GitHub");
        offered.version = Some(offered_version.to_string());
        offered.install_spec = Some(spec.clone());
        cache.upsert_many(&[offered.clone()]).await.unwrap();

        let mut at_install = offered;
        at_install.version = Some(installed_version.to_string());
        cache
            .record_origin(&crate::hub::origin::InstallOrigin::record(
                &at_install,
                &spec,
                "aleph-hub_github",
                0,
            ))
            .await
            .unwrap();
        cache
    }

    #[tokio::test]
    async fn installed_list_gets_the_update_badge_from_the_ledger() {
        let cache = seeded_cache("2.0.0", "1.0.0").await;
        let mut installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        stamp_updates_from_ledger(&mut installed, &cache).await;
        assert!(
            installed[0].update_available,
            "installed panel must see the newer catalog version"
        );
    }

    #[tokio::test]
    async fn installed_list_badge_stays_dark_at_the_same_version() {
        let cache = seeded_cache("1.0.0", "1.0.0").await;
        let mut installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        stamp_updates_from_ledger(&mut installed, &cache).await;
        assert!(!installed[0].update_available);
    }

    /// An installed extension the ledger never recorded (installed by hand, or
    /// before the ledger existed) makes no claim.
    #[tokio::test]
    async fn unrecorded_backend_gets_no_badge() {
        let cache = seeded_cache("2.0.0", "1.0.0").await;
        let mut installed = vec![installed_entry(
            "local:mcp:something-else",
            ExtensionKind::Mcp,
            "Other",
            true,
        )];
        stamp_updates_from_ledger(&mut installed, &cache).await;
        assert!(!installed[0].update_available);
    }
}
