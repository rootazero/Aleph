use crate::gateway::security::SharedTokenManager;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendDto {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    /// `SearXNG` only — comma-separated upstream engines to pin (e.g. "bing").
    /// Maps to [`crate::config::types::SearchBackendConfig::engines`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,
    /// Reported on get (never echoes the secret); ignored on update.
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub verified: bool,
    /// Mirrors [`crate::config::types::SearchBackendConfig::allow_private_upstream`].
    ///
    /// On this face because this face is where `base_url` is SET, and for
    /// SearXNG it is required. Without it, a self-hosted instance added
    /// through the Panel would be accepted, silently dropped at boot by the
    /// SSRF host check, and unfixable from the interface that created it —
    /// the gate would have an opener that only `config.toml` could reach
    /// (判据 §9: one verb, two faces, one derivation).
    #[serde(default)]
    pub allow_private_upstream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigDto {
    pub enabled: bool,
    pub default_provider: String,
    pub max_results: u64,
    pub timeout_seconds: u64,
    pub pii_enabled: bool,
    pub pii_scrub_email: bool,
    pub pii_scrub_phone: bool,
    pub pii_scrub_ssn: bool,
    pub pii_scrub_credit_card: bool,
    #[serde(default)]
    pub backends: Vec<SearchBackendDto>,
}

/// Vault key prefix for search provider API keys
pub(crate) fn vault_key(backend_name: &str) -> String {
    format!("search:{backend_name}")
}

/// Resolve API key from vault for a search backend
pub(crate) fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    crate::gateway::handlers::resolve_vault_secret(&vault_key(name), vault)
}
