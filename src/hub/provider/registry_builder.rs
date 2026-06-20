//! Assemble the v1 `ProviderRegistry` (official MCP registry + Docker catalog +
//! plugin marketplaces).

use crate::extension::marketplace::types::MarketplaceConfig;
use crate::extension::marketplace::MarketplaceManager;
use crate::hub::provider::docker_mcp::DockerMcpProvider;
use crate::hub::provider::marketplace::MarketplaceProvider;
use crate::hub::provider::mcp_registry::McpRegistryProvider;
use crate::hub::provider::static_hub::StaticHubProvider;
use crate::hub::provider::ProviderRegistry;
use crate::hub::types::TrustTier;
use std::collections::HashMap;

/// Built-in Aleph Hub catalog artifact URL. Placeholder pending the Aleph-Hub
/// deployment (separate Next.js/Vercel project). Until it is live, sync fails
/// gracefully (Network error) and the source stays empty.
const ALEPH_HUB_URL: &str = "https://hub.aleph.computer/catalog.json";

pub fn build_default_registry(
    marketplaces: HashMap<String, MarketplaceConfig>,
) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    reg.register(Box::new(McpRegistryProvider::new()));
    reg.register(Box::new(DockerMcpProvider::new()));
    reg.register(Box::new(MarketplaceProvider {
        manager: MarketplaceManager::new(marketplaces, None),
        provider_id: "cc-marketplace".into(),
    }));
    // Built-in Aleph Hub: centrally-curated catalog carrying full provenance.
    reg.register(Box::new(StaticHubProvider::new(
        "aleph-hub".into(),
        "Aleph Hub".into(),
        ALEPH_HUB_URL.into(),
        TrustTier::Verified,
    )));
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_registers_providers() {
        let reg = build_default_registry(HashMap::new());
        assert!(reg.get("mcp-official").is_some());
        assert!(reg.get("docker-mcp").is_some());
        assert!(reg.get("cc-marketplace").is_some());
        assert!(reg.get("aleph-hub").is_some());
    }
}
