//! `extensions.catalog` (cached, filtered, offline-capable) and
//! `extensions.installed` (live reconciliation across MCP / plugins / skills).

use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::skills::shared_system;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::mcp::manager::McpManagerHandle;
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::reconcile::{mcp_to_entry, plugin_to_entry, skill_to_entry};
use crate::hub::types::{ExtensionCategory, ExtensionKind};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct CatalogParams {
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    pub query: Option<String>,
}

/// extensions.catalog — filtered read of the cached catalog (offline-capable).
pub async fn handle_catalog(req: JsonRpcRequest, cache: Arc<CatalogCache>) -> JsonRpcResponse {
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
        Ok(entries) => JsonRpcResponse::success(req.id, json!({ "extensions": entries })),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// extensions.installed — live reconciled list across all backends.
pub async fn handle_installed(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
) -> JsonRpcResponse {
    let mut out = Vec::new();

    if let Some(mcp) = &mcp {
        match mcp.list_servers().await {
            Ok(servers) => out.extend(servers.iter().map(mcp_to_entry)),
            Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("mcp: {e}")),
        }
    }

    if let Some(mgr) = crate::extension::try_extension_manager() {
        if let Err(e) = mgr.ensure_loaded().await {
            tracing::warn!("extensions.installed: failed to load plugins: {e}");
        }
        out.extend(mgr.list_plugin_records().await.iter().map(plugin_to_entry));
    }

    out.extend(
        shared_system()
            .full_status()
            .await
            .iter()
            .map(skill_to_entry),
    );

    JsonRpcResponse::success(req.id, json!({ "extensions": out }))
}
