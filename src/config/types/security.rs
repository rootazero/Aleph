//! Shell security configuration types
//!
//! Controls command risk assessment with optional custom patterns.
//! All built-in patterns remain active as safety floor regardless of config.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// CustomPattern
// =============================================================================

/// A user-defined regex pattern for command risk classification.
///
/// Custom patterns are **additive** — built-in patterns remain active
/// as a safety floor even when custom patterns are enabled.
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
/// When `enable_custom_patterns` is `false` (default), only built-in
/// patterns are used — behavior is identical to pre-configuration.
///
/// When `true`, custom patterns are evaluated *in addition to* built-ins.
/// Built-in patterns always win (they are checked first).
///
/// # Example (aleph.toml)
///
/// ```toml
/// [security.shell]
/// enable_custom_patterns = true
///
/// [[security.shell.custom_blocked]]
/// pattern = "^dangerous_tool\\s+"
/// reason = "Custom blocked tool"
///
/// [[security.shell.custom_danger]]
/// pattern = "^custom_admin_cmd\\s+"
/// reason = "Requires approval"
///
/// [[security.shell.custom_safe]]
/// pattern = "^my_safe_script\\s+"
/// reason = "Auto-approved"
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

    /// Custom safe patterns (additive to built-ins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_safe: Vec<CustomRiskPattern>,
}

fn default_false() -> bool {
    false
}

impl ShellSecurityConfig {
    /// Compile all enabled custom patterns into Regex.
    ///
    /// Returns `Ok(None)` if custom patterns are disabled.
    /// Returns `Ok(Some(...))` with compiled patterns if enabled.
    pub fn compile_patterns(&self) -> Result<Option<CompiledCustomPatterns>, regex::Error> {
        if !self.enable_custom_patterns {
            return Ok(None);
        }

        let compile = |patterns: &[CustomRiskPattern]| -> Result<Vec<Regex>, regex::Error> {
            patterns
                .iter()
                .map(|p| Regex::new(&p.pattern))
                .collect::<Result<Vec<_>, _>>()
        };

        Ok(Some(CompiledCustomPatterns {
            blocked: compile(&self.custom_blocked)?,
            danger: compile(&self.custom_danger)?,
            safe: compile(&self.custom_safe)?,
        }))
    }
}

/// Pre-compiled custom patterns for runtime use.
#[derive(Debug, Clone)]
pub struct CompiledCustomPatterns {
    pub blocked: Vec<Regex>,
    pub danger: Vec<Regex>,
    pub safe: Vec<Regex>,
}

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
        assert!(config.custom_safe.is_empty());
    }

    #[test]
    fn test_compile_disabled() {
        let config = ShellSecurityConfig::default();
        assert!(config.compile_patterns().unwrap().is_none());
    }

    #[test]
    fn test_compile_enabled() {
        let config = ShellSecurityConfig {
            enable_custom_patterns: true,
            custom_blocked: vec![CustomRiskPattern {
                pattern: r"^rm\s+".to_string(),
                reason: Some("Delete".to_string()),
            }],
            custom_danger: vec![CustomRiskPattern {
                pattern: r"^sudo\s+".to_string(),
                reason: None,
            }],
            custom_safe: vec![],
        };

        let compiled = config.compile_patterns().unwrap().unwrap();
        assert_eq!(compiled.blocked.len(), 1);
        assert_eq!(compiled.danger.len(), 1);
        assert!(compiled.safe.is_empty());

        assert!(compiled.blocked[0].is_match("rm -rf /"));
        assert!(compiled.danger[0].is_match("sudo ls"));
    }

    #[test]
    fn test_compile_invalid_regex() {
        let config = ShellSecurityConfig {
            enable_custom_patterns: true,
            custom_blocked: vec![CustomRiskPattern {
                pattern: "[invalid".to_string(),
                reason: None,
            }],
            ..Default::default()
        };

        assert!(config.compile_patterns().is_err());
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
