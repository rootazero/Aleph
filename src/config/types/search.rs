//! Search configuration types
//!
//! Contains search capability configuration:
//! - `SearchConfigInternal`: Internal search config with `HashMap` backends
//! - `SearchBackendConfig`: Individual search backend settings
//! - `PIIConfig`: PII scrubbing settings

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
    #[serde(default)]
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

    /// PII scrubbing configuration (migrate from `behavior.pii_scrubbing_enabled`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PIIConfig>,

    /// Enable the WebFetch-based SERP scrape fallback (Round-2 feature).
    ///
    /// When `true` (default), [`crate::search::SearchRegistry`] will
    /// scrape no-credential mirrors (DDG Lite, DDG HTML) after every
    /// configured provider has failed. This rescues the search tool
    /// during "all paid APIs are simultaneously rate-limited" outages
    /// without operator intervention.
    ///
    /// Set to `false` only in environments where outbound HTTP to
    /// duckduckgo.com is policy-blocked or the operator wants
    /// hard-fail behaviour for auditability.
    ///
    /// Backward compatible: existing TOML configs without this key
    /// pick up the default (enabled) on next load.
    #[serde(default = "default_web_fetch_fallback")]
    pub web_fetch_fallback: bool,
}

/// Default for `SearchConfigInternal.web_fetch_fallback` — enabled.
///
/// Round-2 chose default-on because the operational pain (search
/// silently fails until quotas reset) is severe and the privacy
/// posture (DDG over HTTPS, no API key) is no worse than the existing
/// DDG provider users already have access to.
pub const fn default_web_fetch_fallback() -> bool {
    true
}

pub const fn default_search_max_results() -> usize {
    5
}

pub const fn default_search_timeout() -> u64 {
    10
}

// =============================================================================
// PIIConfig
// =============================================================================

/// PII (Personally Identifiable Information) scrubbing configuration
///
/// Migrated from `behavior.pii_scrubbing_enabled` to search.pii
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

pub const fn default_true() -> bool {
    true
}

pub const fn default_false() -> bool {
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
    /// Provider type: "tavily", "searxng", "brave", "google", "bing", "exa",
    /// "jina", "duckduckgo", or "firecrawl"
    pub provider_type: String,

    /// Runtime-only API key (populated from encrypted vault, never persisted to config.toml)
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,

    /// Base URL (required for `SearXNG`, optional for others)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Search engine ID (required for Google CSE only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,

    /// `SearXNG` only — comma-separated upstream engines to query
    /// (e.g. "bing,baidu,360search"). Pins requests to rate-tolerant engines
    /// so a burst of agent searches doesn't trigger CAPTCHA / rate-limit
    /// suspension on sensitive engines (brave/duckduckgo). When unset, the
    /// `SearXNG` instance's default engine set is used. Ignored by other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,

    /// `SearXNG` only — minimum interval between requests to this backend, in
    /// milliseconds. Throttles request rate so rate-sensitive upstream engines
    /// don't get suspended under a burst. Defaults to 2000ms (empirically tuned)
    /// when unset; set to 0 to disable throttling. Ignored by other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_request_interval_ms: Option<u64>,

    /// Whether this backend has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,
}
