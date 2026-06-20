//! `extensions.sources.*` — list configured source providers and trigger a
//! catalog refresh (concurrent `sync_all_into`).

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::hub::cache::CatalogCache;
use crate::hub::provider::ProviderRegistry;
use serde_json::json;
use std::sync::Arc;

/// extensions.sources.list — provider ids, trust tiers, and kinds (from cache-free metadata).
pub async fn handle_list(req: JsonRpcRequest, reg: Arc<ProviderRegistry>) -> JsonRpcResponse {
    let sources: Vec<_> = reg
        .list_sources()
        .into_iter()
        .map(|(id, tier, kinds)| {
            json!({
                "id": id,
                "trust_tier": tier.as_str(),
                "kinds": kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect();
    JsonRpcResponse::success(req.id, json!({ "sources": sources }))
}

/// extensions.sources.refresh — sync every provider into the cache; report counts.
pub async fn handle_refresh(
    req: JsonRpcRequest,
    reg: Arc<ProviderRegistry>,
    cache: Arc<CatalogCache>,
) -> JsonRpcResponse {
    let report = reg.sync_all_into(&cache).await;
    JsonRpcResponse::success(
        req.id,
        json!({
            "synced": report.synced.iter().map(|(id, n)| json!({"source": id, "count": n})).collect::<Vec<_>>(),
            "failed": report.failed.iter().map(|(id, e)| json!({"source": id, "error": e})).collect::<Vec<_>>(),
        }),
    )
}
