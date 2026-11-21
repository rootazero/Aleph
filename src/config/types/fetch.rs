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
///
/// The section holds the fetch BACKENDS an operator configured (base URL,
/// vault-backed token, timeout) and backs the `fetch_config.test`
/// connection check. It deliberately carries NO routing knobs: `web_fetch`
/// never delegates to a fetch provider (BT-D-R4-22 — the SSRF DNS pin
/// cannot be enforced on a provider-side crawl), so an `enabled` flag or a
/// `default_provider` would only ever be a lie an operator could read as an
/// active setting. Boot logs a one-time warning for configured backends
/// (see `inert_fetch_backend_names`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FetchConfigInternal {
    /// Backend configurations, keyed by provider name.
    #[serde(default)]
    pub backends: HashMap<String, FetchBackendConfig>,

    /// Verified flag for shared (search-backed) providers that DO NOT land
    /// in `backends` (currently just firecrawl, which shares the
    /// `[search].backends.firecrawl` config and `search:firecrawl` vault
    /// key per Strategy V). Without this side-channel, a successful
    /// `fetch_config.test` for firecrawl would never persist `verified`,
    /// and the DTO returned by `handle_get` would always report
    /// `verified: false` for firecrawl even after a confirmed-good test.
    /// Keyed by provider name; empty by default.
    #[serde(default)]
    pub verified_shared: HashMap<String, bool>,
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
            backends,
            verified_shared: HashMap::new(),
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back: FetchConfigInternal = toml::from_str(&toml).unwrap();
        assert_eq!(
            back.backends["crawl4ai"].base_url.as_deref(),
            Some("http://x:11235")
        );
        assert!(back.backends["crawl4ai"].verified);
    }
}
