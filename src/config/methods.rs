//! Configuration utility methods
//!
//! This module provides methods for managing providers, rules, and tools configuration.

use crate::config::types::UnifiedToolsConfig;
use crate::config::Config;
use crate::error::{AlephError, Result};
use tracing::{debug, error};

impl Config {
    // =============================================================================
    // Tools Configuration Methods
    // =============================================================================

    /// Get effective tools configuration (unified format)
    ///
    /// This method provides a unified view of tools configuration:
    /// - If `unified_tools` is present, it takes precedence
    /// - Otherwise, creates unified config from legacy `tools` + `mcp` sections
    ///
    /// This enables gradual migration from legacy config format to unified format.
    #[must_use]
    pub fn get_effective_tools_config(&self) -> UnifiedToolsConfig {
        if let Some(unified) = &self.unified_tools {
            unified.clone()
        } else {
            UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)
        }
    }

    // =============================================================================
    // Provider Management Methods
    // =============================================================================

    /// Set the default provider with validation
    ///
    /// Validates that:
    /// - Provider exists in providers map
    /// - Provider is enabled
    ///
    /// # Arguments
    /// * `name` - The name of the provider to set as default
    ///
    /// # Returns
    /// * `Ok(())` - Successfully set default provider
    /// * `Err(AlephError::InvalidConfig)` - Provider not found or disabled
    pub fn set_default_provider(&mut self, name: &str) -> Result<()> {
        match self.providers.get(name) {
            Some(config) if config.enabled => {
                debug!(provider = %name, "Setting default provider");
                self.general.default_provider = Some(name.to_string());
                Ok(())
            }
            Some(_) => {
                error!(provider = %name, "Cannot set disabled provider as default");
                Err(AlephError::invalid_config(format!(
                    "Provider '{name}' is not enabled"
                )))
            }
            None => {
                error!(provider = %name, "Provider not found in config");
                Err(AlephError::invalid_config(format!(
                    "Provider '{name}' not found"
                )))
            }
        }
    }

    // =============================================================================
    // Routing Rule Management Methods
    // =============================================================================

}
