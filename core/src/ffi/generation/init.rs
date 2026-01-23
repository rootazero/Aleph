//! Generation provider initialization
//!
//! This module contains the initialization function for generation providers.

use crate::config::Config;
use crate::generation::GenerationProviderRegistry;
use std::sync::Arc;
use tracing::{info, warn};

/// Initialize generation providers from configuration
pub(crate) fn init_generation_providers(
    config: &Config,
) -> Arc<std::sync::RwLock<GenerationProviderRegistry>> {
    use crate::generation::providers::create_provider;

    let mut registry = GenerationProviderRegistry::new();

    // Iterate over configured generation providers
    for (name, provider_config) in &config.generation.providers {
        if !provider_config.enabled {
            info!(provider = %name, "Generation provider disabled, skipping");
            continue;
        }

        match create_provider(name, provider_config) {
            Ok(provider) => {
                if let Err(e) = registry.register(name.clone(), provider) {
                    warn!(provider = %name, error = %e, "Failed to register generation provider");
                } else {
                    info!(provider = %name, "Registered generation provider");
                }
            }
            Err(e) => {
                warn!(provider = %name, error = %e, "Failed to create generation provider");
            }
        }
    }

    info!(
        provider_count = registry.len(),
        "Generation provider registry initialized"
    );

    Arc::new(std::sync::RwLock::new(registry))
}
