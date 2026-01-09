//! Search operations for AetherCore
//!
//! This module contains all search capability methods:
//! - Search provider testing
//! - Search configuration updates
//! - Search registry management

use super::AetherCore;
use crate::error::{AetherError, Result};
use std::sync::Arc;
use tracing::{info, warn};

impl AetherCore {
    // ========================================================================
    // SEARCH CAPABILITY METHODS (integrate-search-registry)
    // ========================================================================

    /// Extract SearchOptions from search configuration (integrate-search-registry)
    ///
    /// Converts SearchConfigInternal to SearchOptions for use in capability executor.
    ///
    /// # Arguments
    ///
    /// * `search_config` - Search configuration from Config
    ///
    /// # Returns
    ///
    /// * `crate::search::SearchOptions` - Configured search options
    pub(crate) fn get_search_options_from_config(
        search_config: &crate::config::SearchConfigInternal,
    ) -> crate::search::SearchOptions {
        use crate::search::SearchOptions;

        // Create SearchOptions with defaults, override from config
        SearchOptions {
            max_results: search_config.max_results,
            timeout_seconds: search_config.timeout_seconds,
            // Use default values for other fields (None or false)
            ..Default::default()
        }
    }

    /// Create SearchRegistry from search configuration (integrate-search-registry)
    ///
    /// This method initializes a SearchRegistry with configured backends and fallback chain.
    ///
    /// # Arguments
    ///
    /// * `search_config` - Search configuration from Config
    ///
    /// # Returns
    ///
    /// * `Result<crate::search::SearchRegistry>` - Initialized registry or error
    pub(crate) fn create_search_registry_from_config(
        search_config: &crate::config::SearchConfigInternal,
    ) -> Result<crate::search::SearchRegistry> {
        use crate::search::providers::*;
        use crate::search::SearchProvider;

        info!(
            enabled = search_config.enabled,
            default_provider = %search_config.default_provider,
            backend_count = search_config.backends.len(),
            "Creating SearchRegistry from config"
        );

        // Create providers from backend configurations
        let mut providers: Vec<(String, Box<dyn SearchProvider>)> = Vec::new();

        for (name, backend_config) in &search_config.backends {
            let provider: Box<dyn SearchProvider> = match backend_config.provider_type.as_str() {
                "tavily" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Tavily requires api_key"))?;
                    Box::new(TavilyProvider::new(api_key.clone())?)
                }
                "searxng" => {
                    let base_url = backend_config
                        .base_url
                        .as_ref()
                        .ok_or_else(|| AetherError::config("SearXNG requires base_url"))?;
                    Box::new(SearxngProvider::new(base_url.clone())?)
                }
                "google" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Google CSE requires api_key"))?;
                    let engine_id = backend_config
                        .engine_id
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Google CSE requires engine_id"))?;
                    Box::new(GoogleProvider::new(api_key.clone(), engine_id.clone())?)
                }
                "bing" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Bing requires api_key"))?;
                    Box::new(BingProvider::new(api_key.clone())?)
                }
                "brave" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Brave requires api_key"))?;
                    Box::new(BraveProvider::new(api_key.clone())?)
                }
                "exa" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Exa requires api_key"))?;
                    Box::new(ExaProvider::new(api_key.clone())?)
                }
                _ => {
                    warn!(
                        provider_type = %backend_config.provider_type,
                        "Unknown search provider type, skipping"
                    );
                    continue;
                }
            };

            providers.push((name.clone(), provider));
        }

        if providers.is_empty() {
            return Err(AetherError::config(
                "No search providers configured in backends",
            ));
        }

        // Build fallback chain
        let fallback_chain = search_config
            .fallback_providers
            .clone()
            .unwrap_or_default();

        // Create registry
        let mut registry =
            crate::search::SearchRegistry::new(search_config.default_provider.clone());

        // Add all providers
        let provider_count = providers.len();
        for (name, provider) in providers {
            // Provider is already Box<dyn SearchProvider>, wrap in Arc
            registry.add_provider(name, Arc::from(provider));
        }

        // Set fallback providers
        registry.set_fallback_providers(fallback_chain);

        info!(
            provider_count = provider_count,
            "SearchRegistry created successfully"
        );

        Ok(registry)
    }

    /// Update search configuration
    ///
    /// Updates the search configuration and reinitializes the SearchRegistry.
    /// This allows hot-reloading search providers after settings changes.
    ///
    /// # Arguments
    /// * `search` - New search configuration (UniFFI type)
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_search_config(&self, search: crate::config::SearchConfig) -> Result<()> {
        // Convert UniFFI SearchConfig to internal SearchConfigInternal
        let search_internal: crate::config::SearchConfigInternal = search.into();

        // Update config and save to disk
        {
            let mut config = self.lock_config();
            config.search = Some(search_internal.clone());
            config.save()?;
        }

        // Reinitialize SearchRegistry with new config
        if search_internal.enabled {
            match Self::create_search_registry_from_config(&search_internal) {
                Ok(registry) => {
                    let mut registry_lock = self
                        .search_registry
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    *registry_lock = Some(Arc::new(registry));
                    log::info!("Search configuration updated and registry reinitialized");
                }
                Err(e) => {
                    log::warn!("Failed to reinitialize SearchRegistry: {}", e);
                    return Err(AetherError::config(format!(
                        "Failed to reinitialize search registry: {}",
                        e
                    )));
                }
            }
        } else {
            // Disable search by clearing the registry
            let mut registry_lock = self
                .search_registry
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *registry_lock = None;
            log::info!("Search capability disabled");
        }

        Ok(())
    }

    /// Test a search provider connection (integrate-search-registry)
    ///
    /// This method delegates to SearchRegistry.test_search_provider() to validate
    /// provider configuration and connectivity. Results are cached for 5 minutes.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the search provider to test
    ///
    /// # Returns
    ///
    /// * `ProviderTestResult` - Test result with success status, latency, and error details
    pub fn test_search_provider(
        &self,
        provider_name: String,
    ) -> Result<crate::search::ProviderTestResult> {
        use crate::search::ProviderTestResult;

        // Clone Arc from registry (must drop lock before await)
        let registry_arc = {
            let registry_guard = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
            registry_guard.as_ref().map(Arc::clone)
        }; // Lock is dropped here

        match registry_arc {
            Some(reg) => {
                // Execute async search test within tokio runtime
                Ok(self
                    .runtime
                    .block_on(reg.test_search_provider(&provider_name)))
            }
            None => {
                // Search capability not enabled
                Ok(ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message: "Search capability not enabled in configuration".to_string(),
                    error_type: "config".to_string(),
                })
            }
        }
    }

    /// Test a search provider with ad-hoc configuration
    ///
    /// This method allows testing provider credentials without requiring the provider
    /// to be saved in the configuration file. It creates a temporary provider instance
    /// to validate connectivity and credentials.
    ///
    /// # Arguments
    ///
    /// * `config` - Ad-hoc configuration containing provider type and credentials
    ///
    /// # Returns
    ///
    /// * `ProviderTestResult` - Test result with success status, latency, and error details
    pub fn test_search_provider_with_config(
        &self,
        config: crate::search::SearchProviderTestConfig,
    ) -> Result<crate::search::ProviderTestResult> {
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

        // Execute test search within tokio runtime
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
                let error_message = e.to_string();
                let error_type = if error_message.contains("401")
                    || error_message.contains("403")
                    || error_message.contains("unauthorized")
                    || error_message.contains("invalid")
                {
                    "auth"
                } else if error_message.contains("timeout")
                    || error_message.contains("connection")
                    || error_message.contains("network")
                {
                    "network"
                } else {
                    "unknown"
                };

                Ok(ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message,
                    error_type: error_type.to_string(),
                })
            }
        }
    }
}
