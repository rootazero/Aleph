//! Configuration utility methods
//!
//! This module provides methods for managing providers, rules, and tools configuration.

use crate::config::Config;
use crate::config::types::{RoutingRuleConfig, UnifiedToolsConfig};
use crate::error::{AetherError, Result};
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
    pub fn get_effective_tools_config(&self) -> UnifiedToolsConfig {
        if let Some(unified) = &self.unified_tools {
            unified.clone()
        } else {
            UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)
        }
    }

    /// Check if using new unified tools configuration
    pub fn is_using_unified_tools(&self) -> bool {
        self.unified_tools.is_some()
    }

    // =============================================================================
    // Provider Management Methods
    // =============================================================================

    /// Get the default provider if it exists and is enabled
    ///
    /// Returns None if:
    /// - No default provider is configured
    /// - Default provider does not exist in providers map
    /// - Default provider is disabled
    ///
    /// # Returns
    /// * `Some(String)` - The name of the enabled default provider
    /// * `None` - No valid default provider
    pub fn get_default_provider(&self) -> Option<String> {
        self.general.default_provider.as_ref().and_then(|name| {
            self.providers.get(name).and_then(|config| {
                if config.enabled {
                    Some(name.clone())
                } else {
                    None
                }
            })
        })
    }

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
    /// * `Err(AetherError::InvalidConfig)` - Provider not found or disabled
    pub fn set_default_provider(&mut self, name: &str) -> Result<()> {
        match self.providers.get(name) {
            Some(config) if config.enabled => {
                debug!(provider = %name, "Setting default provider");
                self.general.default_provider = Some(name.to_string());
                Ok(())
            }
            Some(_) => {
                error!(provider = %name, "Cannot set disabled provider as default");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' is not enabled",
                    name
                )))
            }
            None => {
                error!(provider = %name, "Provider not found in config");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' not found",
                    name
                )))
            }
        }
    }

    /// Get list of all enabled provider names
    ///
    /// Returns provider names in alphabetical order
    ///
    /// # Returns
    /// * `Vec<String>` - List of enabled provider names
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .providers
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        providers.sort();
        providers
    }

    // =============================================================================
    // Routing Rule Management Methods
    // =============================================================================

    /// Add a new routing rule at the top of the list (highest priority)
    ///
    /// New rules are inserted at index 0 to give them the highest priority
    /// in the first-match-stops routing algorithm.
    ///
    /// # Arguments
    /// * `rule` - The routing rule configuration to add
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::{Config, RoutingRuleConfig};
    /// let mut config = Config::default();
    /// config.add_rule_at_top(RoutingRuleConfig {
    ///     regex: r"^\[VSCode\]".to_string(),
    ///     provider: "claude".to_string(),
    ///     system_prompt: Some("You are a coding assistant.".to_string()),
    /// });
    /// // This rule now has highest priority (index 0)
    /// ```
    pub fn add_rule_at_top(&mut self, rule: RoutingRuleConfig) {
        self.rules.insert(0, rule);
        debug!(
            rules_count = self.rules.len(),
            "Added rule at top (highest priority)"
        );
    }

    /// Remove a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to remove (0-based)
    ///
    /// # Returns
    /// * `Ok(())` - Rule removed successfully
    /// * `Err(AetherError::InvalidConfig)` - Index out of bounds
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Assuming rule exists at index 0
    /// config.remove_rule(0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn remove_rule(&mut self, index: usize) -> Result<()> {
        if index < self.rules.len() {
            let removed = self.rules.remove(index);
            debug!(
                index = index,
                rule_type = %removed.get_rule_type(),
                regex = %removed.regex,
                rules_count = self.rules.len(),
                "Removed routing rule"
            );
            Ok(())
        } else {
            error!(
                index = index,
                max_index = self.rules.len().saturating_sub(1),
                "Rule index out of bounds"
            );
            Err(AetherError::invalid_config(format!(
                "Rule index {} out of bounds (valid range: 0-{})",
                index,
                self.rules.len().saturating_sub(1)
            )))
        }
    }

    /// Move a routing rule from one position to another
    ///
    /// This allows reordering rules to change their priority.
    ///
    /// # Arguments
    /// * `from` - Current index of the rule
    /// * `to` - Target index for the rule
    ///
    /// # Returns
    /// * `Ok(())` - Rule moved successfully
    /// * `Err(AetherError::InvalidConfig)` - Invalid indices
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Move rule from index 2 to index 0 (highest priority)
    /// config.move_rule(2, 0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.rules.len() {
            error!(
                from_index = from,
                max_index = self.rules.len().saturating_sub(1),
                "Source rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Source index {} out of bounds (valid range: 0-{})",
                from,
                self.rules.len().saturating_sub(1)
            )));
        }
        if to >= self.rules.len() {
            error!(
                to_index = to,
                max_index = self.rules.len().saturating_sub(1),
                "Target rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Target index {} out of bounds (valid range: 0-{})",
                to,
                self.rules.len().saturating_sub(1)
            )));
        }

        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        debug!(from = from, to = to, "Moved routing rule");
        Ok(())
    }

    /// Get a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to retrieve (0-based)
    ///
    /// # Returns
    /// * `Some(&RoutingRuleConfig)` - Reference to the rule if found
    /// * `None` - Index out of bounds
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// let config = Config::default();
    /// if let Some(rule) = config.get_rule(0) {
    ///     println!("First rule: {}", rule.regex);
    /// }
    /// ```
    pub fn get_rule(&self, index: usize) -> Option<&RoutingRuleConfig> {
        self.rules.get(index)
    }

    /// Get the number of routing rules
    ///
    /// # Returns
    /// * `usize` - Number of routing rules configured
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}
