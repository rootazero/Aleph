//! `hub_catalog_sync` — run all provider syncs into the local catalog cache.
//! Categorization (Task 1) runs inside sync_all_into, so this also refreshes
//! functional categories. The deterministic curation entry point.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::extension::marketplace::types::MarketplaceConfig;
use crate::hub::cache::CatalogCache;
use crate::hub::provider::registry_builder::build_default_registry;
use crate::hub::provider::SyncReport;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StoreCatalogSyncArgs {}

#[derive(Debug, Clone, Serialize)]
pub struct StoreCatalogSyncOutput {
    pub synced: Vec<(String, usize)>,
    pub failed: Vec<(String, String)>,
}

impl StoreCatalogSyncOutput {
    #[must_use]
    pub fn from_report(r: &SyncReport) -> Self {
        Self {
            synced: r.synced.clone(),
            failed: r.failed.clone(),
        }
    }
}

#[derive(Clone)]
pub struct HubCatalogSyncTool {
    pub cache: Arc<CatalogCache>,
    pub marketplaces: HashMap<String, MarketplaceConfig>,
}

#[async_trait]
impl AlephTool for HubCatalogSyncTool {
    const NAME: &'static str = "hub_catalog_sync";
    const DESCRIPTION: &'static str =
        "Sync all extension sources into the local catalog cache and refresh functional categories.";
    type Args = StoreCatalogSyncArgs;
    type Output = StoreCatalogSyncOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let registry = build_default_registry(self.marketplaces.clone());
        let report = registry.sync_all_into(&self.cache).await;
        Ok(StoreCatalogSyncOutput::from_report(&report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::provider::SyncReport;

    #[test]
    fn output_from_report() {
        let rep = SyncReport {
            synced: vec![("mcp-official".into(), 12)],
            failed: vec![("docker-mcp".into(), "timeout".into())],
        };
        let out = StoreCatalogSyncOutput::from_report(&rep);
        assert_eq!(out.synced, rep.synced);
        assert_eq!(out.failed.len(), 1);
    }
}
