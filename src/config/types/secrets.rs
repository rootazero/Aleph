//! Secret provider configuration types
//!
//! These types support the secrets subsystem configuration:
//! - `SecretProviderConfig`: backend configuration (local vault, 1Password, Bitwarden, etc.)
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
    /// Backend type: "`local_vault`", "1password", "bitwarden", etc.
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
// VirtualKeyMapping
// =============================================================================

/// Maps a virtual/alias key name to an actual secret name.
///
/// This allows users to use shorter, stable aliases in prompts while
/// the actual secret name can change. Useful for team-shared configs
/// where different members may have different secret naming conventions.
///
/// # Example (aleph.toml)
///
/// ```toml
/// [secrets_config.virtual_keys]
/// "openai" = "OPENAI_API_KEY"
/// "anthropic" = "ANTHROPIC_API_KEY_PROD"
/// ```
///
/// Then in prompts: `{{secret:openai}}` resolves to the secret
/// mapped by `OPENAI_API_KEY`.
pub type VirtualKeyMap = std::collections::HashMap<String, String>;

// =============================================================================
// CustomLeakPattern
// =============================================================================

/// A user-defined leak detection pattern.
///
/// Custom patterns are evaluated alongside built-in patterns for outbound
/// and inbound leak scanning. Invalid regex patterns are logged and skipped.
///
/// # Example (aleph.toml)
///
/// ```toml
/// [[secrets_config.custom_leak_patterns]]
/// name = "Internal API Token"
/// pattern = "internal-[a-z0-9]{32}"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomLeakPattern {
    /// Human-readable name for this pattern (used in logs/block reasons)
    pub name: String,
    /// Regex pattern to match potential secrets
    pub pattern: String,
}

// =============================================================================
// SecretsConfig
// =============================================================================

/// Top-level settings for the secrets subsystem
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Virtual key aliases: alias -> actual secret name
    #[serde(default, skip_serializing_if = "VirtualKeyMap::is_empty")]
    pub virtual_keys: VirtualKeyMap,
    /// Custom leak detection patterns (additive to built-ins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_leak_patterns: Vec<CustomLeakPattern>,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            virtual_keys: VirtualKeyMap::new(),
            custom_leak_patterns: Vec::new(),
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
        assert!(config.virtual_keys.is_empty());
        assert!(config.custom_leak_patterns.is_empty());
    }

    #[test]
    fn test_secrets_config_virtual_keys() {
        let toml_str = r#"
            [virtual_keys]
            "openai" = "OPENAI_API_KEY"
            "anthropic" = "ANTHROPIC_KEY"
        "#;
        let config: SecretsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.virtual_keys.len(), 2);
        assert_eq!(
            config.virtual_keys.get("openai"),
            Some(&"OPENAI_API_KEY".to_string())
        );
        assert_eq!(
            config.virtual_keys.get("anthropic"),
            Some(&"ANTHROPIC_KEY".to_string())
        );
    }

    #[test]
    fn test_secrets_config_custom_leak_patterns() {
        let toml_str = r#"
            [[custom_leak_patterns]]
            name = "Internal API Token"
            pattern = "internal-[a-z0-9]{32}"

            [[custom_leak_patterns]]
            name = "Service Key"
            pattern = "svc-[A-Z]{8}"
        "#;
        let config: SecretsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.custom_leak_patterns.len(), 2);
        assert_eq!(config.custom_leak_patterns[0].name, "Internal API Token");
        assert_eq!(
            config.custom_leak_patterns[0].pattern,
            "internal-[a-z0-9]{32}"
        );
        assert_eq!(config.custom_leak_patterns[1].name, "Service Key");
    }
}
