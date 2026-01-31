//! SecurityKernel - Deterministic command risk assessment.
//!
//! Uses regex pattern matching for zero-latency security decisions.
//! Does NOT rely on LLM for security judgments.

use super::risk::{RiskLevel, BLOCKED_PATTERNS, DANGER_PATTERNS, SAFE_PATTERNS};
use regex::Regex;

/// Security kernel for command risk assessment.
///
/// # Example
///
/// ```rust
/// use aethecore::exec::SecurityKernel;
///
/// let kernel = SecurityKernel::default();
///
/// // Blocked command
/// let risk = kernel.assess("rm -rf /");
/// assert!(risk.is_blocked());
///
/// // Safe command
/// let risk = kernel.assess("ls -la");
/// assert_eq!(risk, aethecore::exec::RiskLevel::Safe);
/// ```
#[derive(Debug, Clone)]
pub struct SecurityKernel {
    /// Custom blocked patterns (in addition to defaults)
    custom_blocked: Vec<Regex>,
    /// Custom danger patterns (in addition to defaults)
    custom_danger: Vec<Regex>,
    /// Custom safe patterns (in addition to defaults)
    custom_safe: Vec<Regex>,
}

impl Default for SecurityKernel {
    fn default() -> Self {
        Self {
            custom_blocked: Vec::new(),
            custom_danger: Vec::new(),
            custom_safe: Vec::new(),
        }
    }
}

impl SecurityKernel {
    /// Create a new security kernel with default patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom blocked pattern.
    pub fn add_blocked_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_blocked.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Add a custom danger pattern.
    pub fn add_danger_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_danger.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Add a custom safe pattern.
    pub fn add_safe_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.custom_safe.push(Regex::new(pattern)?);
        Ok(())
    }

    /// Assess the risk level of a command.
    ///
    /// Evaluation order (first match wins):
    /// 1. Blocked patterns → RiskLevel::Blocked
    /// 2. Danger patterns → RiskLevel::Danger
    /// 3. Safe patterns → RiskLevel::Safe
    /// 4. Default → RiskLevel::Caution
    pub fn assess(&self, command: &str) -> RiskLevel {
        let cmd = command.trim();

        // 1. Check blocked patterns (custom first, then defaults)
        for pattern in self.custom_blocked.iter().chain(BLOCKED_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Blocked;
            }
        }

        // 2. Check danger patterns
        for pattern in self.custom_danger.iter().chain(DANGER_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Danger;
            }
        }

        // 3. Check safe patterns
        for pattern in self.custom_safe.iter().chain(SAFE_PATTERNS.iter()) {
            if pattern.is_match(cmd) {
                return RiskLevel::Safe;
            }
        }

        // 4. Default: Caution (unknown commands)
        RiskLevel::Caution
    }

    /// Assess a command and return detailed result.
    pub fn assess_detailed(&self, command: &str) -> RiskAssessment {
        let level = self.assess(command);
        let reason = match level {
            RiskLevel::Blocked => "Command matches blocked pattern",
            RiskLevel::Danger => "Command matches danger pattern",
            RiskLevel::Caution => "Command is unknown, requires caution",
            RiskLevel::Safe => "Command matches safe pattern",
        };

        RiskAssessment {
            command: command.to_string(),
            level,
            reason: reason.to_string(),
        }
    }
}

/// Detailed risk assessment result.
#[derive(Debug, Clone)]
pub struct RiskAssessment {
    /// The assessed command
    pub command: String,
    /// Risk level
    pub level: RiskLevel,
    /// Human-readable reason
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assess_blocked() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("rm -rf /"), RiskLevel::Blocked);
        assert_eq!(kernel.assess("rm -rf /*"), RiskLevel::Blocked);
    }

    #[test]
    fn test_assess_danger() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("rm -rf ./temp"), RiskLevel::Danger);
        assert_eq!(kernel.assess("sudo apt install vim"), RiskLevel::Danger);
        assert_eq!(kernel.assess("chmod 755 script.sh"), RiskLevel::Danger);
    }

    #[test]
    fn test_assess_safe() {
        let kernel = SecurityKernel::new();
        assert_eq!(kernel.assess("ls -la"), RiskLevel::Safe);
        assert_eq!(kernel.assess("echo hello"), RiskLevel::Safe);
        assert_eq!(kernel.assess("git status"), RiskLevel::Safe);
        assert_eq!(kernel.assess("pwd"), RiskLevel::Safe);
        assert_eq!(kernel.assess("cat file.txt"), RiskLevel::Safe);
    }

    #[test]
    fn test_assess_caution() {
        let kernel = SecurityKernel::new();
        // Unknown commands default to Caution
        assert_eq!(kernel.assess("npm install"), RiskLevel::Caution);
        assert_eq!(kernel.assess("cargo build"), RiskLevel::Caution);
        assert_eq!(kernel.assess("docker run nginx"), RiskLevel::Caution);
    }

    #[test]
    fn test_custom_blocked_pattern() {
        let mut kernel = SecurityKernel::new();
        kernel.add_blocked_pattern(r"^danger-cmd").unwrap();
        assert_eq!(kernel.assess("danger-cmd arg"), RiskLevel::Blocked);
    }

    #[test]
    fn test_custom_safe_pattern() {
        let mut kernel = SecurityKernel::new();
        kernel.add_safe_pattern(r"^my-safe-tool").unwrap();
        assert_eq!(kernel.assess("my-safe-tool --help"), RiskLevel::Safe);
    }

    #[test]
    fn test_assess_detailed() {
        let kernel = SecurityKernel::new();
        let result = kernel.assess_detailed("ls -la");
        assert_eq!(result.level, RiskLevel::Safe);
        assert!(result.reason.contains("safe"));
    }

    #[test]
    fn test_blocked_takes_priority() {
        let kernel = SecurityKernel::new();
        // Even if it looks like a safe command, blocked patterns win
        assert_eq!(kernel.assess("rm -rf /"), RiskLevel::Blocked);
    }
}
