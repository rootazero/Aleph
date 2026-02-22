//! Privacy configuration types
//!
//! Controls PII (Personally Identifiable Information) filtering behavior
//! in the Provider layer. Each PII category can be independently configured
//! to block, warn, or ignore detected sensitive data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// PiiAction
// =============================================================================

/// Action to take when PII of a specific category is detected
///
/// - `Block`: Replace detected PII with a placeholder before sending to API (default for most categories)
/// - `Warn`: Allow the message but emit a warning event
/// - `Off`: No action, PII passes through unfiltered
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    Block,
    Warn,
    Off,
}

// =============================================================================
// PrivacyConfig
// =============================================================================

/// Privacy and PII filtering configuration
///
/// Controls which categories of personally identifiable information are
/// filtered before messages reach AI providers. Each category has an
/// independent action level.
///
/// # Example Configuration (config.toml)
///
/// ```toml
/// [privacy]
/// pii_filtering = true
/// id_card = "block"
/// bank_card = "block"
/// phone = "block"
/// api_key = "block"
/// ssh_key = "block"
/// email = "warn"
/// ip_address = "off"
/// exclude_providers = ["ollama"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrivacyConfig {
    /// Master switch for PII filtering (default: true)
    #[serde(default = "default_true")]
    pub pii_filtering: bool,

    /// Action for national ID / identity card numbers
    #[serde(default = "default_block")]
    pub id_card: PiiAction,

    /// Action for bank card / credit card numbers
    #[serde(default = "default_block")]
    pub bank_card: PiiAction,

    /// Action for phone numbers
    #[serde(default = "default_block")]
    pub phone: PiiAction,

    /// Action for API keys / tokens
    #[serde(default = "default_block")]
    pub api_key: PiiAction,

    /// Action for SSH private keys
    #[serde(default = "default_block")]
    pub ssh_key: PiiAction,

    /// Action for email addresses
    #[serde(default = "default_warn")]
    pub email: PiiAction,

    /// Action for IP addresses
    #[serde(default = "default_off")]
    pub ip_address: PiiAction,

    /// Provider names excluded from PII filtering
    /// Messages routed to these providers bypass all PII checks.
    #[serde(default)]
    pub exclude_providers: Vec<String>,
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_true() -> bool {
    true
}

fn default_block() -> PiiAction {
    PiiAction::Block
}

fn default_warn() -> PiiAction {
    PiiAction::Warn
}

fn default_off() -> PiiAction {
    PiiAction::Off
}

// =============================================================================
// Default Implementation
// =============================================================================

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            pii_filtering: default_true(),
            id_card: default_block(),
            bank_card: default_block(),
            phone: default_block(),
            api_key: default_block(),
            ssh_key: default_block(),
            email: default_warn(),
            ip_address: default_off(),
            exclude_providers: Vec::new(),
        }
    }
}

impl Default for PiiAction {
    fn default() -> Self {
        PiiAction::Block
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_privacy_config() {
        let config = PrivacyConfig::default();
        assert!(config.pii_filtering);
        assert_eq!(config.id_card, PiiAction::Block);
        assert_eq!(config.bank_card, PiiAction::Block);
        assert_eq!(config.phone, PiiAction::Block);
        assert_eq!(config.api_key, PiiAction::Block);
        assert_eq!(config.ssh_key, PiiAction::Block);
        assert_eq!(config.email, PiiAction::Warn);
        assert_eq!(config.ip_address, PiiAction::Off);
        assert!(config.exclude_providers.is_empty());
    }

    #[test]
    fn test_default_pii_action() {
        assert_eq!(PiiAction::default(), PiiAction::Block);
    }

    #[test]
    fn test_pii_action_serialization() {
        assert_eq!(
            serde_json::to_string(&PiiAction::Block).unwrap(),
            "\"block\""
        );
        assert_eq!(
            serde_json::to_string(&PiiAction::Warn).unwrap(),
            "\"warn\""
        );
        assert_eq!(
            serde_json::to_string(&PiiAction::Off).unwrap(),
            "\"off\""
        );
    }

    #[test]
    fn test_pii_action_deserialization() {
        assert_eq!(
            serde_json::from_str::<PiiAction>("\"block\"").unwrap(),
            PiiAction::Block
        );
        assert_eq!(
            serde_json::from_str::<PiiAction>("\"warn\"").unwrap(),
            PiiAction::Warn
        );
        assert_eq!(
            serde_json::from_str::<PiiAction>("\"off\"").unwrap(),
            PiiAction::Off
        );
    }

    #[test]
    fn test_config_deserialization_with_defaults() {
        let toml_str = r#"
            pii_filtering = true
            email = "block"
        "#;
        let config: PrivacyConfig = toml::from_str(toml_str).unwrap();
        assert!(config.pii_filtering);
        assert_eq!(config.email, PiiAction::Block); // overridden
        assert_eq!(config.id_card, PiiAction::Block); // default
        assert_eq!(config.ip_address, PiiAction::Off); // default
    }

    #[test]
    fn test_config_deserialization_empty() {
        let toml_str = "";
        let config: PrivacyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config, PrivacyConfig::default());
    }

    #[test]
    fn test_exclude_providers() {
        let toml_str = r#"
            exclude_providers = ["ollama", "local-llm"]
        "#;
        let config: PrivacyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.exclude_providers, vec!["ollama", "local-llm"]);
    }
}
