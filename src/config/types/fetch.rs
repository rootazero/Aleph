//! Fetch configuration types — the URL→markdown capability, parallel to
//! `search.rs`. A provider may also be a search provider (e.g. Firecrawl);
//! the Firecrawl fetch backend shares the `[search]` Firecrawl config and
//! vault key, so its `base_url`/`api_key` here stay `None`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// FetchConfigInternal
// =============================================================================

/// Fetch module configuration (parallel to `SearchConfigInternal`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FetchConfigInternal {
    /// Enable routing `web_fetch` through a configured fetch provider.
    /// Off → built-in reqwest+readability only (zero behavior change).
    #[serde(default)]
    pub enabled: bool,

    /// Preferred provider name (key into `backends`).
    #[serde(default)]
    pub default_provider: String,

    /// Providers tried in order if the default fails (before the built-in fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,

    /// Backend configurations, keyed by provider name.
    #[serde(default)]
    pub backends: HashMap<String, FetchBackendConfig>,
}

// =============================================================================
// FetchBackendConfig
// =============================================================================

/// Fetch backend configuration (parallel to `SearchBackendConfig`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchBackendConfig {
    /// Provider type: "crawl4ai" | "firecrawl".
    pub provider_type: String,

    /// Runtime-only token (from vault; never persisted to config.toml).
    /// `None` for shared providers (firecrawl reuses `search:firecrawl`).
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,

    /// Base URL of the backend server. `None` for shared providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Request timeout in seconds (provider default when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Verified via a successful Test connection.
    #[serde(default)]
    pub verified: bool,

    /// Operator gate to disable this backend without removing its config.
    /// Mirrors `SearchBackendConfig::enabled` so the registry can skip a
    /// backend without depending on its `base_url`/`api_key` being empty.
    #[serde(default = "default_backend_enabled")]
    pub enabled: bool,
}

fn default_backend_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_config_omits_token_on_serialize() {
        let b = FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("secret-token".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(60),
            verified: false,
            enabled: true,
        };
        let toml = toml::to_string(&b).unwrap();
        assert!(!toml.contains("secret-token"), "token must never serialize");
        assert!(toml.contains("crawl4ai"));
    }

    #[test]
    fn fetch_config_round_trips_backends() {
        let mut backends = std::collections::HashMap::new();
        backends.insert(
            "crawl4ai".to_string(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: true,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back: FetchConfigInternal = toml::from_str(&toml).unwrap();
        assert_eq!(back.default_provider, "crawl4ai");
        assert_eq!(
            back.backends["crawl4ai"].base_url.as_deref(),
            Some("http://x:11235")
        );
        assert!(back.backends["crawl4ai"].verified);
    }
}
