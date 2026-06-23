//! Unified cold-start primer for the `aleph-hub` catalog slot.
//!
//! Composes the official MCP, skill, and plugin projections into a single
//! `replace_source` so none clobbers the others (the slot is replace-based).
//! Runs only when the slot is empty (never fetched); the async remote fetch
//! later overwrites the slot wholesale.

use crate::hub::cache::CatalogCache;
use crate::hub::catalog_client::ALEPH_HUB_ID;

/// Cold-start primer: if the `aleph-hub` slot is empty (never fetched), fill it
/// with the official MCP + skill + plugin projections so official extensions are
/// available offline. The async remote fetch later `replace_source`s the slot.
pub async fn prime_official_catalog_if_empty(cache: &CatalogCache) {
    match cache.count_source(ALEPH_HUB_ID).await {
        Ok(0) => {
            let mut entries = crate::hub::official_mcp::primer_entries();
            entries.extend(crate::hub::official_skills::primer_entries());
            entries.extend(crate::hub::official_plugins::primer_entries());
            match cache.replace_source(ALEPH_HUB_ID, &entries).await {
                Ok(()) => tracing::info!(
                    count = entries.len(),
                    "primed official catalog (cold start: MCP + skills + plugins)"
                ),
                Err(e) => tracing::warn!(error = %e, "failed to prime official catalog"),
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "count_source failed; skipping official catalog primer")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::cache::CatalogFilter;
    use crate::hub::types::ExtensionKind;

    #[tokio::test]
    async fn primes_when_empty_then_is_noop_when_populated() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        let after = cache
            .query(&CatalogFilter { source_id: Some(ALEPH_HUB_ID.into()), ..Default::default() })
            .await
            .unwrap();
        // MCP catalog.json is always present, so the slot is non-empty even when
        // the skills submodule is absent.
        assert!(after.iter().any(|e| e.id == "aleph-hub:context7"));
        let count = after.len();
        // Second call is a no-op (slot already non-empty).
        prime_official_catalog_if_empty(&cache).await;
        let again = cache
            .query(&CatalogFilter { source_id: Some(ALEPH_HUB_ID.into()), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(again.len(), count);
    }

    #[tokio::test]
    async fn skills_extension_does_not_clobber_mcp() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        let mcp = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Mcp), ..Default::default() })
            .await
            .unwrap();
        // The full MCP primer set survives composition with the skills projection.
        assert_eq!(mcp.len(), crate::hub::official_mcp::primer_entries().len());
        assert!(mcp.iter().all(|e| e.kind == ExtensionKind::Mcp));
    }

    #[tokio::test]
    async fn plugins_compose_without_clobbering_mcp() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        // The full MCP set survives the three-way composition (catalog.json anchor).
        let mcp = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Mcp), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(mcp.len(), crate::hub::official_mcp::primer_entries().len());
        // Any plugin entries primed are well-formed and live in the aleph-hub slot.
        let plugins = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Plugin), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(plugins.len(), crate::hub::official_plugins::primer_entries().len());
        for p in &plugins {
            assert_eq!(p.source_id, ALEPH_HUB_ID);
            assert_eq!(p.trust_tier, crate::hub::types::TrustTier::Official);
        }
    }
}
