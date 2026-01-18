//! Configuration management methods for AetherCore
//!
//! This module contains config-related methods: reload_config, update_provider, etc.

use super::{AetherCore, AetherFfiError, AgentConfigHolder};
use crate::agent::RigAgentConfig;
use crate::config::{
    Config, FullConfig, GeneralConfig, ProviderConfig, RoutingRuleConfig, TestConnectionResult,
};
use std::path::Path;
use tracing::info;

impl AetherCore {
    /// Reload configuration from file
    ///
    /// Re-loads config from the original config path and updates the internal
    /// configuration. If reload fails, the existing configuration remains unchanged.
    ///
    /// # Returns
    /// * `Ok(())` - Configuration reloaded successfully
    /// * `Err(AetherFfiError::Config)` - Failed to load or parse config file
    pub fn reload_config(&self) -> Result<(), AetherFfiError> {
        info!(path = %self.config_path, "Reloading config");

        // Load config from stored path (same logic as init_core)
        let full_config = if self.config_path.is_empty() {
            // Use default path (~/.config/aether/config.toml)
            Config::load().map_err(|e| AetherFfiError::Config(e.to_string()))?
        } else {
            let path = Path::new(&self.config_path);
            if path.exists() {
                Config::load_from_file(path).map_err(|e| AetherFfiError::Config(e.to_string()))?
            } else {
                return Err(AetherFfiError::Config(format!(
                    "Config file not found: {}",
                    self.config_path
                )));
            }
        };

        // Extract provider settings (same logic as init_core)
        let (provider, model, api_key, base_url, system_prompt, temperature, max_tokens) = {
            let default_provider = full_config.get_default_provider();
            if let Some(ref name) = default_provider {
                if let Some(provider_config) = full_config.providers.get(name) {
                    let provider_type = provider_config.infer_provider_type(name);
                    (
                        provider_type,
                        provider_config.model.clone(),
                        provider_config.api_key.clone(),
                        provider_config.base_url.clone(),
                        None::<String>,
                        provider_config.temperature,
                        provider_config.max_tokens,
                    )
                } else {
                    info!(provider = %name, "Default provider config not found, using defaults");
                    (
                        "openai".to_string(),
                        "gpt-4o".to_string(),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                }
            } else {
                info!("No default provider configured, using openai defaults");
                (
                    "openai".to_string(),
                    "gpt-4o".to_string(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            }
        };

        // Create new RigAgentConfig with loaded values
        let new_config = RigAgentConfig {
            provider,
            model,
            temperature: temperature.unwrap_or(0.7),
            max_tokens: max_tokens.unwrap_or(4096),
            max_turns: 50, // Default to 50 turns for complex multi-step tasks
            system_prompt: system_prompt
                .unwrap_or_else(|| "You are Aether, an intelligent assistant.".to_string()),
            api_key,
            base_url,
        };

        info!(
            provider = %new_config.provider,
            model = %new_config.model,
            has_api_key = new_config.api_key.is_some(),
            has_base_url = new_config.base_url.is_some(),
            "Config reloaded successfully"
        );

        // Update config holder (acquire write lock)
        *self.config_holder.write().unwrap() = AgentConfigHolder::new(new_config);

        // Also update full_config
        *self.full_config.lock().unwrap() = full_config;

        Ok(())
    }

    /// Load configuration and return it in UniFFI-compatible format
    pub fn load_config(&self) -> Result<FullConfig, AetherFfiError> {
        let config = self.lock_config();
        Ok(config.clone().into())
    }

    /// Update provider configuration
    ///
    /// This method updates the provider configuration and automatically refreshes
    /// the agent config holder so changes take effect immediately for conversations.
    pub fn update_provider(
        &self,
        name: String,
        provider: ProviderConfig,
    ) -> Result<(), AetherFfiError> {
        {
            let mut config = self.lock_config();
            config.providers.insert(name.clone(), provider);
            config
                .save()
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        }

        // Refresh config_holder so changes take effect immediately for conversations
        // This is critical: without this, the agent would still use the old API key
        info!(provider = %name, "Provider updated, refreshing agent config");
        self.reload_config()?;

        Ok(())
    }

    /// Delete provider configuration
    ///
    /// This method removes a provider and refreshes the agent config holder.
    /// If the deleted provider was the default, conversations will fail until
    /// a new default is set.
    pub fn delete_provider(&self, name: String) -> Result<(), AetherFfiError> {
        {
            let mut config = self.lock_config();
            config.providers.remove(&name);
            config
                .save()
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        }

        // Refresh config_holder in case the default provider changed
        info!(provider = %name, "Provider deleted, refreshing agent config");
        self.reload_config()?;

        Ok(())
    }

    /// Update generation provider configuration
    ///
    /// Adds or updates a generation provider (image/video/audio/speech) in the config.
    /// The provider will be persisted to config.toml under [generation.providers.<name>].
    /// Also updates the in-memory generation registry so changes take effect immediately.
    pub fn update_generation_provider(
        &self,
        name: String,
        provider: crate::ffi::generation::GenerationProviderConfigFFI,
    ) -> Result<(), AetherFfiError> {
        use crate::generation::providers::create_provider;
        use tracing::{info, warn};

        // Convert FFI type to internal config type
        let internal_config: crate::config::GenerationProviderConfig = provider.into();

        // 1. Save to config file
        {
            let mut config = self.lock_config();
            config
                .generation
                .providers
                .insert(name.clone(), internal_config.clone());
            config
                .save()
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        }

        // 2. Sync to in-memory registry
        if internal_config.enabled {
            match create_provider(&name, &internal_config) {
                Ok(provider_instance) => {
                    let mut registry = self.generation_registry.write().unwrap_or_else(|e| {
                        warn!("Generation registry lock poisoned, recovering");
                        e.into_inner()
                    });
                    // Remove existing if any (for updates)
                    let _ = registry.remove(&name);
                    if let Err(e) = registry.register(name.clone(), provider_instance) {
                        warn!(provider = %name, error = %e, "Failed to register generation provider");
                    } else {
                        info!(provider = %name, "Generation provider registered to registry");
                    }
                }
                Err(e) => {
                    warn!(provider = %name, error = %e, "Failed to create generation provider instance");
                }
            }
        } else {
            // Provider is disabled, remove from registry if exists
            let mut registry = self.generation_registry.write().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });
            let _ = registry.remove(&name);
            info!(provider = %name, "Generation provider disabled, removed from registry");
        }

        Ok(())
    }

    /// Get a single generation provider configuration by name
    ///
    /// Returns the configuration for a specific generation provider,
    /// or None if the provider doesn't exist in the config.
    pub fn get_generation_provider_config(
        &self,
        name: String,
    ) -> Option<crate::ffi::generation::GenerationProviderConfigFFI> {
        let config = self.lock_config();
        config
            .generation
            .providers
            .get(&name)
            .map(|provider_config| provider_config.clone().into())
    }

    /// Delete generation provider configuration
    ///
    /// Removes a generation provider from both the config file and the in-memory registry.
    pub fn delete_generation_provider(&self, name: String) -> Result<(), AetherFfiError> {
        use tracing::{info, warn};

        // 1. Remove from config file
        {
            let mut config = self.lock_config();
            config.generation.providers.remove(&name);
            config
                .save()
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        }

        // 2. Remove from in-memory registry
        {
            let mut registry = self.generation_registry.write().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });
            let _ = registry.remove(&name);
            info!(provider = %name, "Generation provider removed from registry");
        }

        Ok(())
    }

    /// Update routing rules
    ///
    /// This method updates the routing rules in config.
    /// **IMPORTANT**: Preserves builtin rules (is_builtin = true) and only
    /// updates user-defined rules.
    pub fn update_routing_rules(
        &self,
        rules: Vec<RoutingRuleConfig>,
    ) -> Result<(), AetherFfiError> {
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

        info!(
            builtin = merged_rules.iter().filter(|r| r.is_builtin).count(),
            user = merged_rules.iter().filter(|r| !r.is_builtin).count(),
            total = merged_rules.len(),
            "Updating routing rules"
        );

        config.rules = merged_rules;
        config
            .validate()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!("Routing rules updated");
        Ok(())
    }

    /// Update shortcuts configuration
    pub fn update_shortcuts(
        &self,
        shortcuts: crate::config::ShortcutsConfig,
    ) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.shortcuts = Some(shortcuts);
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Shortcuts configuration updated");
        Ok(())
    }

    /// Update behavior configuration
    pub fn update_behavior(
        &self,
        behavior: crate::config::BehaviorConfig,
    ) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.behavior = Some(behavior);
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Behavior configuration updated");
        Ok(())
    }

    /// Update trigger configuration
    pub fn update_trigger_config(
        &self,
        trigger: crate::config::TriggerConfig,
    ) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.trigger = Some(trigger);
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Trigger configuration updated");
        Ok(())
    }

    /// Update general configuration (language preference, etc.)
    pub fn update_general_config(&self, new_config: GeneralConfig) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.general = new_config;
        config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save general config: {}", e)))?;
        Ok(())
    }

    /// Update search configuration
    pub fn update_search_config(
        &self,
        search: crate::config::SearchConfig,
    ) -> Result<(), AetherFfiError> {
        // Convert UniFFI SearchConfig to internal SearchConfigInternal
        let search_internal: crate::config::SearchConfigInternal = search.into();

        let mut config = self.lock_config();
        config.search = Some(search_internal);
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Search configuration updated");
        Ok(())
    }

    /// Update policies configuration
    pub fn update_policies(
        &self,
        policies: crate::config::PoliciesConfig,
    ) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.policies = policies;
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Policies configuration updated");
        Ok(())
    }

    /// Test search provider with ad-hoc configuration
    ///
    /// Tests a search provider without requiring saved configuration.
    /// Used by Settings UI to validate credentials before saving.
    pub fn test_search_provider_with_config(
        &self,
        config: crate::search::SearchProviderTestConfig,
    ) -> Result<crate::search::ProviderTestResult, AetherFfiError> {
        use crate::search::providers::*;
        use crate::search::{ProviderTestResult, SearchOptions, SearchProvider};
        use std::time::Instant;

        // Helper: Create config error result
        fn config_error(msg: &str) -> ProviderTestResult {
            ProviderTestResult {
                success: false,
                latency_ms: 0,
                error_message: msg.to_string(),
                error_type: "config".to_string(),
            }
        }

        // Helper: Extract non-empty string from Option, or return None
        fn get_non_empty(opt: &Option<String>) -> Option<String> {
            opt.as_ref().filter(|s| !s.is_empty()).cloned()
        }

        // Helper macro to reduce boilerplate for provider creation
        macro_rules! create_provider {
            ($provider:ident, $name:expr, $key:expr) => {
                match get_non_empty($key) {
                    Some(key) => match $provider::new(key) {
                        Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                        Err(e) => {
                            return Ok(config_error(&format!(
                                "Failed to create {} provider: {}",
                                $name, e
                            )))
                        }
                    },
                    None => return Ok(config_error(&format!("{} requires an API key", $name))),
                }
            };
        }

        // Create temporary provider based on type
        let provider: Box<dyn SearchProvider> = match config.provider_type.as_str() {
            "tavily" => create_provider!(TavilyProvider, "Tavily", &config.api_key),
            "brave" => create_provider!(BraveProvider, "Brave", &config.api_key),
            "bing" => create_provider!(BingProvider, "Bing", &config.api_key),
            "exa" => create_provider!(ExaProvider, "Exa", &config.api_key),
            "searxng" => match get_non_empty(&config.base_url) {
                Some(base_url) => match SearxngProvider::new(base_url) {
                    Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                    Err(e) => {
                        return Ok(config_error(&format!(
                            "Failed to create SearXNG provider: {}",
                            e
                        )))
                    }
                },
                None => return Ok(config_error("SearXNG requires a base URL")),
            },
            "google" => {
                let api_key = match get_non_empty(&config.api_key) {
                    Some(k) => k,
                    None => return Ok(config_error("Google CSE requires an API key")),
                };
                let engine_id = match get_non_empty(&config.engine_id) {
                    Some(id) => id,
                    None => return Ok(config_error("Google CSE requires an engine ID")),
                };
                match GoogleProvider::new(api_key, engine_id) {
                    Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                    Err(e) => {
                        return Ok(config_error(&format!(
                            "Failed to create Google provider: {}",
                            e
                        )))
                    }
                }
            }
            unknown => return Ok(config_error(&format!("Unknown provider type: {}", unknown))),
        };

        // Execute test search
        let test_options = SearchOptions {
            max_results: 1,
            timeout_seconds: 5,
            ..Default::default()
        };

        let start = Instant::now();
        match self
            .runtime
            .block_on(provider.search("test", &test_options))
        {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u32;
                Ok(ProviderTestResult {
                    success: true,
                    latency_ms: latency,
                    error_message: String::new(),
                    error_type: String::new(),
                })
            }
            Err(e) => {
                let latency = start.elapsed().as_millis() as u32;
                let error_str = e.to_string();
                let error_type = if error_str.contains("auth")
                    || error_str.contains("401")
                    || error_str.contains("403")
                {
                    "auth"
                } else if error_str.contains("network")
                    || error_str.contains("timeout")
                    || error_str.contains("connection")
                {
                    "network"
                } else {
                    "config"
                };
                Ok(ProviderTestResult {
                    success: false,
                    latency_ms: latency,
                    error_message: error_str,
                    error_type: error_type.to_string(),
                })
            }
        }
    }

    /// Validate regex pattern
    pub fn validate_regex(&self, pattern: String) -> Result<bool, AetherFfiError> {
        match regex::Regex::new(&pattern) {
            Ok(_) => Ok(true),
            Err(e) => Err(AetherFfiError::Config(format!("Invalid regex: {}", e))),
        }
    }

    /// Test provider connection with temporary configuration
    ///
    /// This method tests a provider without persisting the configuration to disk.
    /// Useful for "Test Connection" feature in UI before saving the provider.
    pub fn test_provider_connection_with_config(
        &self,
        provider_name: String,
        provider_config: ProviderConfig,
    ) -> TestConnectionResult {
        use crate::providers::create_provider;

        // Create provider instance
        let provider = match create_provider(&provider_name, provider_config) {
            Ok(p) => p,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e.user_friendly_message()),
                };
            }
        };

        // Send test request
        let test_prompt = "Say 'OK' if you can read this.";
        let result = self.runtime.block_on(async {
            provider
                .process(test_prompt, None)
                .await
                .map_err(|e| format!("{}", e))
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

    /// Get the current default provider (if exists and enabled)
    pub fn get_default_provider(&self) -> Option<String> {
        let config = self.lock_config();
        config.get_default_provider()
    }

    /// Set the default provider (validates that provider exists and is enabled)
    ///
    /// This method sets the default provider and refreshes the agent config holder
    /// so conversations immediately use the new provider.
    pub fn set_default_provider(&self, provider_name: String) -> Result<(), AetherFfiError> {
        {
            let mut config = self.lock_config();
            config
                .set_default_provider(&provider_name)
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
            config
                .save()
                .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        }

        // Refresh config_holder so conversations use the new default provider
        info!(provider = %provider_name, "Default provider updated, refreshing agent config");
        self.reload_config()?;

        Ok(())
    }

    /// Get list of all enabled provider names (sorted alphabetically)
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let config = self.lock_config();
        config.get_enabled_providers()
    }

    /// Get current log level
    pub fn get_log_level(&self) -> crate::logging::LogLevel {
        crate::logging::get_log_level()
    }

    /// Set log level
    pub fn set_log_level(&self, level: crate::logging::LogLevel) -> Result<(), AetherFfiError> {
        crate::logging::set_log_level(level);
        info!(level = ?level, "Log level set");
        Ok(())
    }

    /// Get log directory path
    pub fn get_log_directory(&self) -> Result<String, AetherFfiError> {
        crate::logging::get_log_directory()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| AetherFfiError::Config(e.to_string()))
    }
}
