//! Shell security configuration types
//!
//! Controls command risk assessment with optional custom patterns.
//! All built-in patterns remain active as safety floor regardless of config.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// CustomPattern
// =============================================================================

/// A user-defined regex pattern for command risk classification.
///
/// These feed the advisory `SecurityKernel` custom-pattern layer
/// (`SecurityKernel::assess_custom`). The catastrophic hard floor is enforced
/// separately and unconditionally by `sandbox::command_policy` — it does not
/// depend on these patterns being configured.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomRiskPattern {
    /// Regex pattern to match against commands
    pub pattern: String,
    /// Human-readable description (used in logs/audit)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// =============================================================================
// ShellSecurityConfig
// =============================================================================

/// Shell command security configuration.
///
/// When `enable_custom_patterns` is `false` (default), the advisory
/// `SecurityKernel` layer matches nothing and every command passes it through.
/// When `true`, the custom blocked / danger patterns below are evaluated as an
/// additional advisory layer. Either way the catastrophic hard floor
/// (`sandbox::command_policy`) is enforced independently and cannot be disabled.
///
/// # Example (aleph.toml)
///
/// This type is the flat `[security]` table on `Config` (`Config.security`),
/// so the custom-pattern arrays live directly under `[security]` — not under a
/// nested `[security.shell]` sub-table.
///
/// ```toml
/// [security]
/// enable_custom_patterns = true
///
/// [[security.custom_blocked]]
/// pattern = "^dangerous_tool\\s+"
/// reason = "Custom blocked tool"
///
/// [[security.custom_danger]]
/// pattern = "^custom_admin_cmd\\s+"
/// reason = "Requires approval"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShellSecurityConfig {
    /// Enable custom risk patterns (default: false)
    #[serde(default = "default_false")]
    pub enable_custom_patterns: bool,

    /// Custom blocked patterns (additive to built-ins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_blocked: Vec<CustomRiskPattern>,

    /// Custom danger patterns (additive to built-ins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_danger: Vec<CustomRiskPattern>,
}

const fn default_false() -> bool {
    false
}

impl ShellSecurityConfig {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ShellSecurityConfig::default();
        assert!(!config.enable_custom_patterns);
        assert!(config.custom_blocked.is_empty());
        assert!(config.custom_danger.is_empty());
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
            enable_custom_patterns = true

            [[custom_blocked]]
            pattern = "^dangerous_tool\\s+"
            reason = "Custom blocked"

            [[custom_danger]]
            pattern = "^admin_cmd\\s+"
        "#;

        let config: ShellSecurityConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enable_custom_patterns);
        assert_eq!(config.custom_blocked.len(), 1);
        assert_eq!(config.custom_blocked[0].pattern, r"^dangerous_tool\s+");
        assert_eq!(
            config.custom_blocked[0].reason,
            Some("Custom blocked".to_string())
        );
        assert_eq!(config.custom_danger.len(), 1);
        assert_eq!(config.custom_danger[0].reason, None);
    }

    #[test]
    fn test_toml_deserialization_empty() {
        let toml_str = "";
        let config: ShellSecurityConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enable_custom_patterns);
        assert!(config.custom_blocked.is_empty());
    }
}
