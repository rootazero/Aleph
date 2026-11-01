//! `SecurityKernel` — advisory custom-pattern command classifier.
//!
//! A thin, purely-additive advisory layer: it evaluates a command against the
//! user-configured `[security]` custom patterns ONLY. The built-in catastrophic
//! floor lives in [`crate::sandbox::command_policy`] (the undisableable
//! hardline) — this kernel deliberately does not re-enforce it, so the two
//! cannot diverge. Its sole production consumer is
//! [`crate::sandbox::security_kernel_hook::SecurityKernelHook`].

use super::risk::RiskLevel;
use crate::config::types::security::CustomRiskPattern;
use crate::config::types::ShellSecurityConfig;
use regex::Regex;

/// Advisory custom-pattern command classifier.
///
/// # Example
///
/// ```rust
/// use alephcore::exec::SecurityKernel;
///
/// // With no custom patterns configured, nothing matches.
/// let kernel = SecurityKernel::default();
/// assert!(kernel.assess_custom("rm -rf /").is_none());
/// ```
#[derive(Debug, Clone, Default)]
pub struct SecurityKernel {
    /// Custom blocked patterns from `[security].custom_blocked`.
    custom_blocked: Vec<Regex>,
    /// Custom danger patterns from `[security].custom_danger`.
    custom_danger: Vec<Regex>,
}

impl SecurityKernel {
    /// Create a security kernel from configuration, compiling the custom
    /// blocked / danger patterns when `enable_custom_patterns` is set.
    ///
    /// Invalid patterns are skipped instead of aborting the whole kernel — one
    /// bad regex must not silently disable every other user-configured rule.
    ///
    /// A dropped *block* rule means the operator's rule never fires, so the
    /// drops are reported: one aggregated warning naming the count and the
    /// offending patterns, emitted once at the end of this function. The
    /// count cannot be handed back — `from_config` yields the kernel alone —
    /// so that warning is the only channel an operator has.
    pub fn from_config(config: &ShellSecurityConfig) -> Self {
        let mut kernel = Self::default();

        if config.enable_custom_patterns {
            let mut dropped = Vec::new();
            compile_patterns(
                &config.custom_blocked,
                &mut kernel.custom_blocked,
                "custom_blocked",
                &mut dropped,
            );
            compile_patterns(
                &config.custom_danger,
                &mut kernel.custom_danger,
                "custom_danger",
                &mut dropped,
            );
            if !dropped.is_empty() {
                tracing::warn!(
                    count = dropped.len(),
                    patterns = %dropped.join(", "),
                    "security kernel: dropped invalid custom pattern regexes; \
                     the dropped rules are NOT enforced",
                );
            }
        }

        kernel
    }

    /// Assess a command against ONLY the configured custom patterns.
    ///
    /// The built-in floor is intentionally skipped so this stays a purely
    /// additive advisory layer on top of the sandbox command-policy hook,
    /// without re-enforcing — and possibly diverging from — the built-in
    /// hardline. Returns `None` when no custom pattern matches.
    #[must_use]
    pub fn assess_custom(&self, command: &str) -> Option<RiskLevel> {
        let cmd = command.trim();
        if self.custom_blocked.iter().any(|p| p.is_match(cmd)) {
            return Some(RiskLevel::Blocked);
        }
        if self.custom_danger.iter().any(|p| p.is_match(cmd)) {
            return Some(RiskLevel::Danger);
        }
        None
    }
}

/// Compile one kind of configured custom pattern into `into`.
///
/// The two kinds differ only in the target vector and the log wording, so
/// they are compiled by one function: a pattern that fails to compile must
/// be dropped the same way wherever it came from.
///
/// A pattern that does not compile is dropped — and its text recorded in
/// `dropped` — instead of aborting the whole kernel: one bad regex must not
/// silently disable every other user-configured rule.
fn compile_patterns(
    patterns: &[CustomRiskPattern],
    into: &mut Vec<Regex>,
    kind: &str,
    dropped: &mut Vec<String>,
) {
    for pattern in patterns {
        match crate::security::safe_regex::bounded_builder(&pattern.pattern).build() {
            Ok(re) => into.push(re),
            Err(err) => {
                tracing::warn!(
                    kind = %kind,
                    pattern = %pattern.pattern,
                    error = %err,
                    "security kernel: dropping invalid custom regex",
                );
                dropped.push(pattern.pattern.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::security::CustomRiskPattern;

    fn blocked(pattern: &str) -> CustomRiskPattern {
        CustomRiskPattern {
            pattern: pattern.to_string(),
            reason: None,
        }
    }

    #[test]
    fn no_custom_patterns_matches_nothing() {
        let kernel = SecurityKernel::default();
        assert!(kernel.assess_custom("rm -rf /").is_none());
        assert!(kernel.assess_custom("ls -la").is_none());
    }

    #[test]
    fn from_config_compiles_and_matches_custom_blocked() {
        let config = ShellSecurityConfig {
            enable_custom_patterns: true,
            custom_blocked: vec![blocked(r"^danger-cmd")],
            ..Default::default()
        };
        let kernel = SecurityKernel::from_config(&config);
        assert_eq!(
            kernel.assess_custom("danger-cmd arg"),
            Some(RiskLevel::Blocked)
        );
        assert!(kernel.assess_custom("safe-cmd arg").is_none());
    }

    #[test]
    fn disabled_custom_patterns_are_not_loaded() {
        let config = ShellSecurityConfig {
            enable_custom_patterns: false,
            custom_blocked: vec![blocked(r"^danger-cmd")],
            ..Default::default()
        };
        let kernel = SecurityKernel::from_config(&config);
        assert!(
            kernel.assess_custom("danger-cmd arg").is_none(),
            "patterns must be ignored when the flag is off"
        );
    }
}
