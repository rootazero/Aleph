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
// CustomMaskPattern
// =============================================================================

/// An operator-defined regex whose matches are redacted from anything a run
/// echoes back to a human.
///
/// The built-in list (`exec::secret_patterns`) recognises *vendor-shaped*
/// credentials — `sk-…`, `AKIA…`, `ghp_…`, PEM blocks. A credential that does
/// not look like any vendor's (an internal service token, a customer id, a
/// staging password) rode through every redaction leg unchanged, because the
/// only way to add a pattern was a method with no production caller. This is
/// that method's caller.
///
/// Additive only: the vendor floor cannot be disabled from config.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomMaskPattern {
    /// Regex matched against every string leaf on its way to a human.
    pub pattern: String,
    /// What the match is replaced with. Defaults to the same marker the
    /// built-in patterns use, so a bare `pattern = "…"` entry is meaningful.
    #[serde(default = "default_mask_replacement")]
    pub replacement: String,
}

fn default_mask_replacement() -> String {
    "***REDACTED***".to_string()
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

    /// Operator-defined secret shapes to redact, additive to the built-in
    /// vendor list. Deliberately **not** behind `enable_custom_patterns`: that
    /// flag guards an advisory layer that can *block a command*, so it is
    /// opt-in; redacting more is never the risky direction.
    ///
    /// ```toml
    /// [[security.mask_patterns]]
    /// pattern = "ACME-[A-Z0-9]{24}"
    /// replacement = "ACME-***"
    /// ```
    ///
    /// `[security]` is not a live-reload section, so a change here takes effect
    /// on restart — which is what `ReloadImpact::classify` already tells the
    /// operator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mask_patterns: Vec<CustomMaskPattern>,

    /// Fail-closed audit opt-in (default: false).
    ///
    /// When `false`, a full audit channel drops entries (counted, and mirrored
    /// into the table as `audit_log_dropped` rows by the drain) — the audit
    /// pipeline degrades itself rather than the system it watches. When
    /// `true`, producers await channel capacity instead: no entry is lost to
    /// backpressure, but a flooded audit pipeline stalls the paths that
    /// produce audit events (request handlers, the content guard). Choose
    /// `true` when an incomplete trail is worse than a slow one.
    ///
    /// Applied where the audit pipelines are built at boot; like the rest of
    /// `[security]`, not live-reloadable.
    #[serde(default = "default_false")]
    pub audit_block_on_full: bool,
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
        assert!(!config.audit_block_on_full);
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
