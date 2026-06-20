//! Assemble the v1 `ProviderRegistry` (official MCP registry + Docker catalog +
//! plugin marketplaces).

use crate::extension::marketplace::types::MarketplaceConfig;
use crate::extension::marketplace::MarketplaceManager;
use crate::store::provider::docker_mcp::DockerMcpProvider;
use crate::store::provider::marketplace::MarketplaceProvider;
use crate::store::provider::mcp_registry::McpRegistryProvider;
use crate::store::provider::ProviderRegistry;
use std::collections::HashMap;

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
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_has_three_providers() {
        let reg = build_default_registry(HashMap::new());
        assert!(reg.get("mcp-official").is_some());
        assert!(reg.get("docker-mcp").is_some());
        assert!(reg.get("cc-marketplace").is_some());
    }
}
