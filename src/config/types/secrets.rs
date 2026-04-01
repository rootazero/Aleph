//! Secret provider and mapping configuration types
//!
//! These types support late-binding secret resolution:
//! - `SecretProviderConfig`: backend configuration (local vault, 1Password, Bitwarden, etc.)
//! - `Sensitivity`: classification level for a secret
//! - `SecretMapping`: maps a logical secret name to a provider + reference
//! - `SecretsConfig`: top-level defaults for the secrets subsystem
//!
//! Example TOML:
//! ```toml
//! [secrets_config]
//! default_provider = "local"
//!
//! [secret_providers.local]
//! type = "local_vault"
//!
//! [secret_providers.op]
//! type = "1password"
//! account = "my.1password.com"
//! service_account_token_env = "OP_SERVICE_ACCOUNT_TOKEN"
//!
//! [secrets.OPENAI_API_KEY]
//! provider = "op"
//! ref = "OpenAI/api-key"
//! sensitivity = "high"
//! ttl = 1800
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// SecretProviderConfig
// =============================================================================

/// Configuration for a secret provider backend
///
/// Each provider entry describes how to connect to a particular secrets store.
/// The `provider_type` field selects the backend implementation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretProviderConfig {
    /// Backend type: "local_vault", "1password", "bitwarden", etc.
    #[serde(rename = "type")]
    pub provider_type: String,

    /// Account identifier (e.g., 1Password account URL)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,

    /// Environment variable that holds the service account token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_token_env: Option<String>,
}

// =============================================================================
// Sensitivity
// =============================================================================

/// Classification level for a secret
///
/// Determines how aggressively the secret is protected at runtime
/// (e.g., cache duration, redaction depth, audit verbosity).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Normal handling — cached per TTL, standard redaction
    #[default]
    Standard,
    /// Elevated protection — shorter effective cache, deeper redaction, audit trail
    High,
}

// =============================================================================
// SecretMapping
// =============================================================================

/// Maps a logical secret name to a provider and optional reference
///
/// When the runtime needs a secret (e.g., `OPENAI_API_KEY`), it looks up the
/// corresponding `SecretMapping` to decide which provider to query and what
/// reference path to use inside that provider.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretMapping {
    /// Name of the provider (must match a key in `secret_providers`)
    pub provider: String,

    /// Provider-specific reference path (e.g., "OpenAI/api-key" for 1Password)
    /// If omitted, the secret name itself is used as the reference.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,

    /// Sensitivity classification
    #[serde(default)]
    pub sensitivity: Sensitivity,

    /// Cache time-to-live in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_ttl")]
    pub ttl: u64,
}

fn default_ttl() -> u64 {
    3600
}

// =============================================================================
// SecretsConfig
// =============================================================================

/// Top-level settings for the secrets subsystem
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Which provider to use when a SecretMapping omits `provider`
    #[serde(default = "default_provider")]
    pub default_provider: String,
}

fn default_provider() -> String {
    "local".into()
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            default_provider: default_provider(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitivity_default_is_standard() {
        assert_eq!(Sensitivity::default(), Sensitivity::Standard);
    }

    #[test]
    fn test_sensitivity_serde_roundtrip() {
        // Serialize
        let high = Sensitivity::High;
        let json = serde_json::to_string(&high).unwrap();
        assert_eq!(json, "\"high\"");

        let standard = Sensitivity::Standard;
        let json_std = serde_json::to_string(&standard).unwrap();
        assert_eq!(json_std, "\"standard\"");

        // Deserialize back
        let parsed: Sensitivity = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Sensitivity::High);

        let parsed_std: Sensitivity = serde_json::from_str(&json_std).unwrap();
        assert_eq!(parsed_std, Sensitivity::Standard);
    }

    #[test]
    fn test_secret_mapping_defaults() {
        // Only provider is required; everything else should default
        let toml_str = r#"
            provider = "local"
        "#;
        let mapping: SecretMapping = toml::from_str(toml_str).unwrap();
        assert_eq!(mapping.provider, "local");
        assert_eq!(mapping.reference, None);
        assert_eq!(mapping.sensitivity, Sensitivity::Standard);
        assert_eq!(mapping.ttl, 3600);
    }

    #[test]
    fn test_secret_provider_config_serde_toml() {
        let toml_str = r#"
            type = "1password"
            account = "my.1password.com"
            service_account_token_env = "OP_SERVICE_ACCOUNT_TOKEN"
        "#;
        let config: SecretProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider_type, "1password");
        assert_eq!(config.account, Some("my.1password.com".to_string()));
        assert_eq!(
            config.service_account_token_env,
            Some("OP_SERVICE_ACCOUNT_TOKEN".to_string())
        );
    }

    #[test]
    fn test_secret_provider_config_minimal() {
        let toml_str = r#"
            type = "local_vault"
        "#;
        let config: SecretProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider_type, "local_vault");
        assert_eq!(config.account, None);
        assert_eq!(config.service_account_token_env, None);
    }

    #[test]
    fn test_secrets_config_default() {
        let config = SecretsConfig::default();
        assert_eq!(config.default_provider, "local");
    }

    #[test]
    fn test_secrets_config_toml_override() {
        let toml_str = r#"
            default_provider = "op"
        "#;
        let config: SecretsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_provider, "op");
    }

    #[test]
    fn test_secret_mapping_full() {
        let toml_str = r#"
            provider = "op"
            ref = "OpenAI/api-key"
            sensitivity = "high"
            ttl = 1800
        "#;
        let mapping: SecretMapping = toml::from_str(toml_str).unwrap();
        assert_eq!(mapping.provider, "op");
        assert_eq!(mapping.reference, Some("OpenAI/api-key".to_string()));
        assert_eq!(mapping.sensitivity, Sensitivity::High);
        assert_eq!(mapping.ttl, 1800);
    }
}
