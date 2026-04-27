//! Privacy configuration types
//!
//! Controls PII (Personally Identifiable Information) filtering behavior
//! in the Provider layer. Each PII category can be independently configured
//! to block, warn, or ignore detected sensitive data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// PiiAction
// =============================================================================

/// Action to take when PII of a specific category is detected
///
/// - `Block`: Replace detected PII with a placeholder before sending to API (default for most categories)
/// - `Warn`: Allow the message but emit a warning event
/// - `Off`: No action, PII passes through unfiltered
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    #[default]
    Block,
    Warn,
    Off,
}

/// Per-platform override for PII filtering rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformPiiPolicy {
    #[serde(default)]
    pub pii_filtering: Option<bool>,
    #[serde(default)]
    pub id_card: Option<PiiAction>,
    #[serde(default)]
    pub bank_card: Option<PiiAction>,
    #[serde(default)]
    pub phone: Option<PiiAction>,
    #[serde(default)]
    pub api_key: Option<PiiAction>,
    #[serde(default)]
    pub ssh_key: Option<PiiAction>,
    #[serde(default)]
    pub email: Option<PiiAction>,
    #[serde(default)]
    pub ip_address: Option<PiiAction>,
    #[serde(default)]
    pub exclude_providers: Option<Vec<String>>,
}

// =============================================================================
// CustomPiiRule
// =============================================================================

/// A user-defined PII detection rule.
///
/// Custom rules are evaluated alongside built-in rules. They use regex
/// patterns and can be configured with any severity and action level.
///
/// # Example (aleph.toml)
///
/// ```toml
/// [[privacy.custom_rules]]
/// name = "internal_token"
/// pattern = "IT-[A-Z0-9]{16}"
/// placeholder = "[INTERNAL_TOKEN]"
/// severity = "high"
/// action = "block"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CustomPiiRule {
    /// Unique rule identifier (used in config and logs)
    pub name: String,
    /// Regex pattern for detection
    pub pattern: String,
    /// Placeholder text for replacement when blocked
    #[serde(default = "default_placeholder")]
    pub placeholder: String,
    /// Severity level (low, medium, high, critical)
    #[serde(default = "default_medium")]
    pub severity: CustomPiiSeverity,
    /// Default action for this rule (can be overridden per-platform)
    #[serde(default)]
    pub action: PiiAction,
}

/// Severity level for custom PII rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CustomPiiSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl From<CustomPiiSeverity> for crate::pii::engine::PiiSeverity {
    fn from(sev: CustomPiiSeverity) -> Self {
        match sev {
            CustomPiiSeverity::Low => crate::pii::engine::PiiSeverity::Low,
            CustomPiiSeverity::Medium => crate::pii::engine::PiiSeverity::Medium,
            CustomPiiSeverity::High => crate::pii::engine::PiiSeverity::High,
            CustomPiiSeverity::Critical => crate::pii::engine::PiiSeverity::Critical,
        }
    }
}

fn default_placeholder() -> String {
    "[CUSTOM_PII]".to_string()
}

fn default_medium() -> CustomPiiSeverity {
    CustomPiiSeverity::Medium
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

    /// Per-platform PII policy overrides.
    #[serde(default)]
    pub platform_policies: HashMap<String, PlatformPiiPolicy>,
    /// Custom PII detection rules (user-defined regex patterns)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_rules: Vec<CustomPiiRule>,
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
            platform_policies: HashMap::new(),
            custom_rules: Vec::new(),
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
        assert!(config.platform_policies.is_empty());
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
        assert_eq!(serde_json::to_string(&PiiAction::Warn).unwrap(), "\"warn\"");
        assert_eq!(serde_json::to_string(&PiiAction::Off).unwrap(), "\"off\"");
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

    #[test]
    fn test_platform_policy_deserialization() {
        let toml_str = r#"
            [platform_policies.telegram]
            phone = "warn"
            email = "off"
            exclude_providers = ["local-llm"]
        "#;
        let config: PrivacyConfig = toml::from_str(toml_str).unwrap();
        let telegram = config.platform_policies.get("telegram").unwrap();
        assert_eq!(telegram.phone, Some(PiiAction::Warn));
        assert_eq!(telegram.email, Some(PiiAction::Off));
        assert_eq!(
            telegram.exclude_providers,
            Some(vec!["local-llm".to_string()])
        );
    }

    #[test]
    fn test_custom_rule_deserialization() {
        let toml_str = r#"
            [[custom_rules]]
            name = "internal_token"
            pattern = "IT-[A-Z0-9]{16}"
            placeholder = "[INTERNAL_TOKEN]"
            severity = "high"
            action = "block"

            [[custom_rules]]
            name = "employee_id"
            pattern = "EMP-[0-9]{6}"
        "#;
        let config: PrivacyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.custom_rules.len(), 2);
        assert_eq!(config.custom_rules[0].name, "internal_token");
        assert_eq!(config.custom_rules[0].pattern, "IT-[A-Z0-9]{16}");
        assert_eq!(
            config.custom_rules[0].placeholder,
            "[INTERNAL_TOKEN]"
        );
        assert_eq!(
            config.custom_rules[0].severity,
            CustomPiiSeverity::High
        );
        assert_eq!(config.custom_rules[0].action, PiiAction::Block);

        assert_eq!(config.custom_rules[1].name, "employee_id");
        assert_eq!(config.custom_rules[1].placeholder, "[CUSTOM_PII]");
        assert_eq!(
            config.custom_rules[1].severity,
            CustomPiiSeverity::Medium
        );
        assert_eq!(config.custom_rules[1].action, PiiAction::Block);
    }

    #[test]
    fn test_custom_rule_default_values() {
        let rule = CustomPiiRule {
            name: "test".to_string(),
            pattern: r"\d+".to_string(),
            placeholder: default_placeholder(),
            severity: default_medium(),
            action: PiiAction::default(),
        };
        assert_eq!(rule.placeholder, "[CUSTOM_PII]");
        assert_eq!(rule.severity, CustomPiiSeverity::Medium);
        assert_eq!(rule.action, PiiAction::Block);
    }
}
