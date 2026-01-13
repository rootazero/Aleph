//! Configuration operations for AetherCore
//!
//! This module contains all configuration management methods:
//! - Loading and saving configuration
//! - Provider management
//! - Routing rules management
//! - Shortcuts and behavior configuration
//!
//! Uses scoped refresh for incremental updates to improve performance.

use super::tools::RefreshScope;
use super::AetherCore;
use crate::config::{GeneralConfig, TestConnectionResult};
use crate::error::{AetherError, Result};
use crate::router::Router;
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    // ========================================================================
    // CONFIG MANAGEMENT METHODS (Phase 6 - Task 1.5)
    // ========================================================================

    /// Internal helper to test provider connection (shared logic)
    ///
    /// This method contains the common testing logic used by both
    /// `test_provider_connection()` and `test_provider_connection_with_config()`.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider (for error messages)
    /// * `provider_config` - Provider configuration to test
    ///
    /// # Returns
    ///
    /// TestConnectionResult with success status and message
    pub(crate) fn test_provider_internal(
        provider_name: &str,
        provider_config: crate::config::ProviderConfig,
    ) -> TestConnectionResult {
        use crate::providers::create_provider;
        use tokio::runtime::Runtime;

        // Create provider instance
        let provider = match create_provider(provider_name, provider_config) {
            Ok(p) => p,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e.user_friendly_message()),
                };
            }
        };

        // Send test request (block on async operation)
        let test_prompt = "Say 'OK' if you can read this.";
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create runtime: {}", e),
                };
            }
        };

        let result = runtime.block_on(async {
            provider.process(test_prompt, None).await.map_err(|e| {
                // During testing, show detailed error for debugging
                // (unlike production where we show user-friendly messages)
                format!("{}", e)
            })
        });

        match result {
            Ok(response) => TestConnectionResult {
                success: true,
                message: format!(
                    "✓ Connection successful! Provider responded: {}",
                    response.chars().take(50).collect::<String>()
                ),
            },
            Err(err_msg) => TestConnectionResult {
                success: false,
                message: err_msg,
            },
        }
    }

    /// Load configuration and return it in UniFFI-compatible format
    pub fn load_config(&self) -> Result<crate::config::FullConfig> {
        let config = self.lock_config();
        Ok(config.clone().into())
    }

    /// Update provider configuration
    pub fn update_provider(
        &self,
        name: String,
        provider: crate::config::ProviderConfig,
    ) -> Result<()> {
        let mut config = self.lock_config();
        config.providers.insert(name, provider);
        config.save()?;
        Ok(())
    }

    /// Delete provider configuration
    pub fn delete_provider(&self, name: String) -> Result<()> {
        let mut config = self.lock_config();
        config.providers.remove(&name);
        config.save()?;
        Ok(())
    }

    /// Update routing rules
    ///
    /// This method updates the routing rules in config AND reinitializes the router
    /// to ensure the new rules take effect immediately.
    ///
    /// **IMPORTANT**: This method preserves builtin rules (is_builtin = true) and only
    /// updates user-defined rules. Builtin rules are prepended to maintain their priority.
    pub fn update_routing_rules(
        &self,
        rules: Vec<crate::config::RoutingRuleConfig>,
    ) -> Result<()> {
        let mut config = self.lock_config();

        // Preserve builtin rules from current config
        let builtin_rules: Vec<_> = config
            .rules
            .iter()
            .filter(|r| r.is_builtin)
            .cloned()
            .collect();

        // Merge: builtin rules first (for priority), then user rules
        let mut merged_rules = builtin_rules;
        merged_rules.extend(rules);

        log::info!(
            "Updating routing rules: {} builtin + {} user = {} total",
            merged_rules.iter().filter(|r| r.is_builtin).count(),
            merged_rules.iter().filter(|r| !r.is_builtin).count(),
            merged_rules.len()
        );

        config.rules = merged_rules;
        config.validate()?;
        config.save()?;
        drop(config); // Release lock before reloading router

        // Reinitialize router with updated config
        self.reload_router()?;

        // Scoped refresh: only update custom commands from routing rules
        self.refresh_tool_registry_scoped(RefreshScope::CustomCommandsOnly);

        log::info!("Routing rules updated and router reinitialized");
        Ok(())
    }

    /// Reload the router from current configuration
    ///
    /// This method reinitializes the router using the current config.
    /// Called after config changes to ensure routing rules take effect immediately.
    pub fn reload_router(&self) -> Result<()> {
        let config = self.lock_config();

        let new_router = if !config.providers.is_empty() {
            match Router::new(&config) {
                Ok(r) => {
                    log::info!(
                        "Router reloaded with {} rules and {} providers",
                        config.rules.len(),
                        config.providers.len()
                    );
                    Some(Arc::new(r))
                }
                Err(e) => {
                    log::warn!("Failed to reinitialize router: {}", e);
                    return Err(e);
                }
            }
        } else {
            log::warn!("No providers configured, router will be empty");
            None
        };

        drop(config); // Release config lock before acquiring router lock

        // Update router with write lock
        let mut router_guard = self.router.write().unwrap_or_else(|e| e.into_inner());
        *router_guard = new_router;

        Ok(())
    }

    /// Update shortcuts configuration
    pub fn update_shortcuts(&self, shortcuts: crate::config::ShortcutsConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.shortcuts = Some(shortcuts);
        config.save()?;
        log::info!("Shortcuts configuration updated");
        Ok(())
    }

    /// Update behavior configuration
    pub fn update_behavior(&self, behavior: crate::config::BehaviorConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.behavior = Some(behavior);
        config.save()?;
        log::info!("Behavior configuration updated");
        Ok(())
    }

    /// Update trigger configuration
    ///
    /// Updates the trigger configuration for the hotkey system.
    /// This controls how double-tap modifier keys trigger cut/copy operations.
    ///
    /// # Arguments
    /// * `trigger` - New trigger configuration
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_trigger_config(&self, trigger: crate::config::TriggerConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.trigger = Some(trigger);
        config.save()?;
        log::info!("Trigger configuration updated");
        Ok(())
    }

    /// Update general configuration (language preference, etc.)
    ///
    /// This method updates the general configuration section and persists to disk.
    /// Used for settings like language preference that don't require service restart.
    ///
    /// # Arguments
    /// * `new_config` - New general configuration
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_general_config(&self, new_config: GeneralConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.general = new_config;

        // Persist config to disk
        config
            .save()
            .map_err(|e| AetherError::config(format!("Failed to save general config: {}", e)))?;

        Ok(())
    }

    /// Validate regex pattern
    pub fn validate_regex(&self, pattern: String) -> Result<bool> {
        match regex::Regex::new(&pattern) {
            Ok(_) => Ok(true),
            Err(e) => Err(AetherError::invalid_config(format!("Invalid regex: {}", e))),
        }
    }

    /// Test provider connection
    ///
    /// Sends a test request to the provider to verify configuration.
    /// Returns a TestConnectionResult with success status and message.
    pub fn test_provider_connection(&self, provider_name: String) -> TestConnectionResult {
        // Get provider config from stored configuration
        let config = self.lock_config();
        let provider_config = match config.providers.get(&provider_name) {
            Some(cfg) => cfg.clone(),
            None => {
                drop(config);
                return TestConnectionResult {
                    success: false,
                    message: format!("Provider '{}' not found in configuration", provider_name),
                };
            }
        };
        drop(config); // Release lock before async operations

        // Use internal helper
        Self::test_provider_internal(&provider_name, provider_config)
    }

    /// Test provider connection with temporary configuration
    ///
    /// This method tests a provider without persisting the configuration to disk.
    /// Useful for "Test Connection" feature in UI before saving the provider.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider (for logging and error messages)
    /// * `provider_config` - Temporary provider configuration to test
    ///
    /// # Returns
    ///
    /// TestConnectionResult with success status and message
    pub fn test_provider_connection_with_config(
        &self,
        provider_name: String,
        provider_config: crate::config::ProviderConfig,
    ) -> TestConnectionResult {
        // Use internal helper directly
        Self::test_provider_internal(&provider_name, provider_config)
    }

    // DEFAULT PROVIDER MANAGEMENT METHODS (Phase 3.3 - add-default-provider-selection)

    /// Get the current default provider (if exists and enabled)
    ///
    /// Returns None if:
    /// - No default provider is configured
    /// - Default provider does not exist
    /// - Default provider is disabled
    pub fn get_default_provider(&self) -> Option<String> {
        let config = self.lock_config();
        config.get_default_provider()
    }

    /// Set the default provider (validates that provider exists and is enabled)
    ///
    /// # Arguments
    /// * `provider_name` - The name of the provider to set as default
    ///
    /// # Returns
    /// * `Ok(())` - Successfully set default provider
    /// * `Err` - Provider not found or disabled
    pub fn set_default_provider(&self, provider_name: String) -> Result<()> {
        let mut config = self.lock_config();
        config.set_default_provider(&provider_name)?;
        config.save()?;

        // Notify event handler of config change
        self.event_handler.on_config_changed();

        info!(provider = %provider_name, "Default provider updated");
        Ok(())
    }

    /// Get list of all enabled provider names (sorted alphabetically)
    ///
    /// # Returns
    /// * `Vec<String>` - List of enabled provider names
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let config = self.lock_config();
        config.get_enabled_providers()
    }
}
