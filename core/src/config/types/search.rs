//! Search configuration types
//!
//! Contains search capability configuration:
//! - SearchConfigInternal: Internal search config with HashMap backends
//! - SearchConfig: UniFFI-compatible search config with Vec backends
//! - SearchBackendConfig: Individual search backend settings
//! - SearchBackendEntry: Backend with name (for UniFFI)
//! - PIIConfig: PII scrubbing settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// SearchConfigInternal
// =============================================================================

/// Search module configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchConfigInternal {
    /// Enable/disable search functionality
    #[serde(default)]
    pub enabled: bool,

    /// Default search provider
    pub default_provider: String,

    /// Fallback providers (tried in order if default fails)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,

    /// Maximum number of results to return (default: 5)
    #[serde(default = "default_search_max_results")]
    pub max_results: usize,

    /// Search timeout in seconds (default: 10)
    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,

    /// Backend configurations
    pub backends: HashMap<String, SearchBackendConfig>,

    /// PII scrubbing configuration (migrate from behavior.pii_scrubbing_enabled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PIIConfig>,
}

pub fn default_search_max_results() -> usize {
    5
}

pub fn default_search_max_results_u64() -> u64 {
    5
}

pub fn default_search_timeout() -> u64 {
    10
}

// =============================================================================
// PIIConfig
// =============================================================================

/// PII (Personally Identifiable Information) scrubbing configuration
///
/// Migrated from behavior.pii_scrubbing_enabled to search.pii
/// (integrate-search-registry proposal)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PIIConfig {
    /// Enable PII scrubbing (email, phone, SSN, etc.)
    #[serde(default)]
    pub enabled: bool,

    /// Scrub email addresses
    #[serde(default = "default_true")]
    pub scrub_email: bool,

    /// Scrub phone numbers
    #[serde(default = "default_true")]
    pub scrub_phone: bool,

    /// Scrub SSN (Social Security Numbers)
    #[serde(default = "default_true")]
    pub scrub_ssn: bool,

    /// Scrub credit card numbers
    #[serde(default = "default_true")]
    pub scrub_credit_card: bool,
}

pub fn default_true() -> bool {
    true
}

pub fn default_false() -> bool {
    false
}

impl Default for PIIConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scrub_email: true,
            scrub_phone: true,
            scrub_ssn: true,
            scrub_credit_card: true,
        }
    }
}

// =============================================================================
// SearchBackendConfig
// =============================================================================

/// Search backend configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchBackendConfig {
    /// Provider type: "tavily", "searxng", "brave", "google", "bing", "exa"
    pub provider_type: String,

    /// API key (required for most providers except SearXNG)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub api_key: Option<String>,

    /// Base URL (required for SearXNG, optional for others)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Search engine ID (required for Google CSE only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,

    /// Whether this backend has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,
}

// =============================================================================
// SearchBackendEntry
// =============================================================================

/// Search backend entry (name + config) - used for UniFFI serialization
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchBackendEntry {
    pub name: String,
    pub config: SearchBackendConfig,
}

// =============================================================================
// SearchConfig (UniFFI)
// =============================================================================

/// Search configuration for UniFFI (backends as Vec instead of HashMap)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchConfig {
    pub enabled: bool,
    pub default_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,
    #[serde(default = "default_search_max_results_u64")]
    pub max_results: u64,
    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,
    pub backends: Vec<SearchBackendEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PIIConfig>,
}

impl From<SearchConfigInternal> for SearchConfig {
    fn from(config: SearchConfigInternal) -> Self {
        let backends = config
            .backends
            .into_iter()
            .map(|(name, config)| SearchBackendEntry { name, config })
            .collect();

        Self {
            enabled: config.enabled,
            default_provider: config.default_provider,
            fallback_providers: config.fallback_providers,
            max_results: config.max_results as u64,
            timeout_seconds: config.timeout_seconds,
            backends,
            pii: config.pii,
        }
    }
}

impl From<SearchConfig> for SearchConfigInternal {
    fn from(config: SearchConfig) -> Self {
        let backends = config
            .backends
            .into_iter()
            .map(|entry| (entry.name, entry.config))
            .collect();

        Self {
            enabled: config.enabled,
            default_provider: config.default_provider,
            fallback_providers: config.fallback_providers,
            max_results: config.max_results as usize,
            timeout_seconds: config.timeout_seconds,
            backends,
            pii: config.pii,
        }
    }
}
