//! Secret provider configuration types
//!
//! These types support the secrets subsystem configuration:
//! - `SecretsConfig`: top-level settings for the secrets subsystem
//!   (virtual keys, custom leak patterns)
//!
//! Example TOML:
//! ```toml
//! [secrets_config.virtual_keys]
//! "openai" = "OPENAI_API_KEY"
//!
//! [[secrets_config.custom_leak_patterns]]
//! name = "Internal API Token"
//! pattern = "internal-[a-z0-9]{32}"
//! ```
//!
//! The `[secret_providers]` table and its `SecretProviderConfig` type were
//! removed in the 2026-09-05 audit pass (secrets I-3): the only backend
//! behind it was a 1Password stub whose trait never grew a `get_secret`,
//! so a configured provider could never resolve a secret. Unknown top-level
//! tables are ignored (`Config` does not `deny_unknown_fields`), so an old
//! config carrying `[secret_providers.*]` keeps loading unchanged.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Virtual key aliases: alias -> actual secret name
    #[serde(default, skip_serializing_if = "VirtualKeyMap::is_empty")]
    pub virtual_keys: VirtualKeyMap,
    /// Custom leak detection patterns (additive to built-ins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_leak_patterns: Vec<CustomLeakPattern>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
