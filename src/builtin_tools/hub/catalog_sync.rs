//! `hub_catalog_sync` — sync the Aleph Hub catalog into the local cache.
//! Uses the standalone `AlephHubCatalog` client (Task 2); no provider registry.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::hub::cache::CatalogCache;
use crate::hub::catalog_client::{AlephHubCatalog, ALEPH_HUB_ID, ALEPH_HUB_NAME, ALEPH_HUB_URL};
use crate::hub::types::TrustTier;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct HubCatalogSyncArgs {}

#[derive(Debug, Clone, Serialize)]
pub struct HubCatalogSyncOutput {
    pub synced: usize,
    pub failed: Vec<String>,
}

#[derive(Clone)]
pub struct HubCatalogSyncTool {
    pub cache: Arc<CatalogCache>,
}

#[async_trait]
impl AlephTool for HubCatalogSyncTool {
    const NAME: &'static str = "hub_catalog_sync";
    const DESCRIPTION: &'static str =
        "Sync the Aleph Hub catalog into the local cache.";
    type Args = HubCatalogSyncArgs;
    type Output = HubCatalogSyncOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let hub = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, ALEPH_HUB_URL, TrustTier::Verified);
        let report = hub.sync_into(&self.cache).await;
        Ok(HubCatalogSyncOutput { synced: report.synced, failed: report.failed })
    }
}
