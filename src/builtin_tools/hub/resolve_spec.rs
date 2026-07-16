//! `hub_resolve_spec` — look up a catalog entry by id and return its cached
//! install spec. No provider registry — resolution is a pure cache lookup of
//! `ExtensionEntry.install_spec`.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HubResolveSpecArgs {
    /// The catalog entry id to resolve (e.g. "mcp-official:io.github.acme/foo").
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubResolveSpecOutput {
    pub entry_id: String,
    pub install_spec: serde_json::Value,
}

#[derive(Clone)]
pub struct HubResolveSpecTool {
    pub cache: Arc<CatalogCache>,
}

#[async_trait]
impl AlephTool for HubResolveSpecTool {
    const NAME: &'static str = "hub_resolve_spec";
    const DESCRIPTION: &'static str =
        "Resolve the install spec for a catalog entry by its id from the local catalog cache.";
    type Args = HubResolveSpecArgs;
    type Output = HubResolveSpecOutput;

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

        // Resolve the install spec from the cached entry (no provider registry).
        let entry_id = args.entry_id;
        let spec = entry
            .install_spec
            .ok_or_else(|| AlephError::other(format!("no install spec cached for {entry_id}")))?;

        let install_spec = serde_json::to_value(&spec)
            .map_err(|e| AlephError::other(format!("serialize InstallSpec failed: {e}")))?;

        Ok(HubResolveSpecOutput {
            entry_id,
            install_spec,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::cache::CatalogCache;
    use crate::hub::types::{
        ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier,
    };

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
            via: None,
            install_spec: None,
        }
    }

    #[tokio::test]
    async fn entry_not_found_returns_error() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let tool = HubResolveSpecTool {
            cache: Arc::new(cache),
        };
        let result = tool
            .call(HubResolveSpecArgs {
                entry_id: "nonexistent:entry".into(),
            })
            .await;
        assert!(result.is_err(), "expected Err for missing entry");
    }

    #[tokio::test]
    async fn returns_cached_install_spec() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let mut e = sample_entry("aleph-hub:foo", "aleph-hub");
        e.install_spec = Some(InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["@t/foo".into()],
            env: vec![],
        });
        cache.upsert_many(&[e]).await.unwrap();
        let tool = HubResolveSpecTool {
            cache: Arc::new(cache),
        };
        let out = tool
            .call(HubResolveSpecArgs {
                entry_id: "aleph-hub:foo".into(),
            })
            .await
            .unwrap();
        let got: InstallSpec = serde_json::from_value(out.install_spec).unwrap();
        assert!(matches!(got, InstallSpec::McpStdio { .. }));
    }

    #[tokio::test]
    async fn errors_when_no_spec_cached() {
        let cache = CatalogCache::open_in_memory().unwrap();
        cache
            .upsert_many(&[sample_entry("aleph-hub:bar", "aleph-hub")])
            .await
            .unwrap();
        let tool = HubResolveSpecTool {
            cache: Arc::new(cache),
        };
        let err = tool
            .call(HubResolveSpecArgs {
                entry_id: "aleph-hub:bar".into(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no install spec cached"));
    }
}
