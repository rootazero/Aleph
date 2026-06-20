//! `store_resolve_spec` — look up a catalog entry by id and resolve its
//! install spec via the matching source provider.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::extension::marketplace::types::MarketplaceConfig;
use crate::store::cache::{CatalogCache, CatalogFilter};
use crate::store::provider::registry_builder::build_default_registry;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StoreResolveSpecArgs {
    /// The catalog entry id to resolve (e.g. "mcp-official:io.github.acme/foo").
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreResolveSpecOutput {
    pub entry_id: String,
    pub install_spec: serde_json::Value,
}

#[derive(Clone)]
pub struct StoreResolveSpecTool {
    pub cache: Arc<CatalogCache>,
    pub marketplaces: HashMap<String, MarketplaceConfig>,
}

#[async_trait]
impl AlephTool for StoreResolveSpecTool {
    const NAME: &'static str = "store_resolve_spec";
    const DESCRIPTION: &'static str =
        "Resolve the install spec for a catalog entry by its id, routing through the matching source provider.";
    type Args = StoreResolveSpecArgs;
    type Output = StoreResolveSpecOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Load the entry from the cache by id.
        let entries = self
            .cache
            .query(&CatalogFilter {
                id: Some(args.entry_id.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| AlephError::other(format!("catalog query failed: {e}")))?;

        let entry = entries.into_iter().next().ok_or_else(|| {
            AlephError::other(format!("entry '{}' not found in catalog", args.entry_id))
        })?;

        // Build the provider registry and resolve the install spec.
        let registry = build_default_registry(self.marketplaces.clone());
        let spec = registry
            .resolve_for_entry(&entry)
            .await
            .map_err(|e| AlephError::other(format!("resolve_for_entry failed: {e}")))?;

        let install_spec = serde_json::to_value(&spec)
            .map_err(|e| AlephError::other(format!("serialize InstallSpec failed: {e}")))?;

        Ok(StoreResolveSpecOutput {
            entry_id: args.entry_id,
            install_spec,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::cache::CatalogCache;
    use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};

    fn sample_entry(id: &str, source_id: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Developer,
            name: "Test Entry".into(),
            description: "A test entry.".into(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: source_id.into(),
            repo_url: None,
            trust_tier: TrustTier::Community,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
        }
    }

    #[tokio::test]
    async fn entry_not_found_returns_error() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let tool = StoreResolveSpecTool {
            cache: Arc::new(cache),
            marketplaces: HashMap::new(),
        };
        let result = tool
            .call(StoreResolveSpecArgs {
                entry_id: "nonexistent:entry".into(),
            })
            .await;
        assert!(result.is_err(), "expected Err for missing entry");
    }

    #[tokio::test]
    async fn known_entry_unknown_provider_returns_error() {
        let cache = CatalogCache::open_in_memory().unwrap();
        // Insert a real entry with a source_id that has no registered provider.
        cache
            .upsert_many(&[sample_entry("local:foo", "local")])
            .await
            .unwrap();
        let tool = StoreResolveSpecTool {
            cache: Arc::new(cache),
            marketplaces: HashMap::new(),
        };
        let result = tool
            .call(StoreResolveSpecArgs {
                entry_id: "local:foo".into(),
            })
            .await;
        // "local" is not a registered provider → resolve_for_entry → Err.
        assert!(result.is_err(), "expected Err when provider missing");
    }
}
