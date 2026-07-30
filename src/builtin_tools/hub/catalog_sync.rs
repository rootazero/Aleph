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
    /// Publish timestamp of the catalog that was ingested, when the fetch got far
    /// enough to parse a manifest. `synced: 0` with a `generated_at` means the
    /// last-good cache was kept; without one, the fetch never landed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

#[derive(Clone)]
pub struct HubCatalogSyncTool {
    pub cache: Arc<CatalogCache>,
}

#[async_trait]
impl AlephTool for HubCatalogSyncTool {
    const NAME: &'static str = "hub_catalog_sync";
    const DESCRIPTION: &'static str =
        "Refresh the local extension catalog from the published Aleph Hub. Browsing works \
         offline from the cache, so this is only needed when results look stale or an expected \
         extension is missing. A failed or empty fetch keeps the last-good cache and reports the \
         reason in `failed`; `generated_at` is the catalog's publish time.";
    type Args = HubCatalogSyncArgs;
    type Output = HubCatalogSyncOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let hub = AlephHubCatalog::new(
            ALEPH_HUB_ID,
            ALEPH_HUB_NAME,
            ALEPH_HUB_URL,
            TrustTier::Verified,
        );
        let report = hub.sync_into(&self.cache).await;
        Ok(HubCatalogSyncOutput {
            synced: report.synced,
            failed: report.failed,
            generated_at: report.generated_at,
        })
    }
}
