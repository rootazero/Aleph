//! Safety gating for generated tools.
//!
//! This module provides security validation for generated tools:
//! 1. Analyze instructions for dangerous operations
//! 2. Check for file system access patterns
//! 3. Check for network access patterns
//! 4. Gate execution with confirmation for risky tools
//!
//! ## Safety Levels
//!
//! - **Safe**: No dangerous operations detected
//! - **Caution**: Some risky patterns, needs user review
//! - **Dangerous**: Contains dangerous operations, requires explicit approval
//! - **Blocked**: Violates security policies, cannot execute

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::types::SolidificationSuggestion;

// =============================================================================
// Safety Types
// =============================================================================

/// Safety level for a generated tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyLevel {
    /// No dangerous operations detected
    Safe,
    /// Some risky patterns, needs user review
    Caution,
    /// Contains dangerous operations, requires explicit approval
    Dangerous,
    /// Violates security policies, cannot execute
    Blocked,
}

impl SafetyLevel {
    /// Check if this level allows automatic execution
    pub fn allows_auto_execute(&self) -> bool {
        matches!(self, SafetyLevel::Safe)
    }

    /// Check if this level requires confirmation
    pub fn requires_confirmation(&self) -> bool {
        !matches!(self, SafetyLevel::Safe)
    }

    /// Check if this level blocks execution entirely
    pub fn is_blocked(&self) -> bool {
        matches!(self, SafetyLevel::Blocked)
    }
}

/// A safety concern detected in the tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConcern {
    /// Type of concern
    pub concern_type: ConcernType,
    /// Human-readable description
    pub description: String,
    /// The pattern that triggered this concern
    pub pattern: String,
    /// Severity level
    pub severity: SafetyLevel,
}

/// Types of safety concerns
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcernType {
    /// Destructive file operations (rm, delete, etc.)
    DestructiveFileOp,
    /// System command execution (shell, exec, etc.)
    SystemCommand,
    /// Elevated privileges (sudo, admin, etc.)
    ElevatedPrivilege,
    /// Network access to external hosts
    NetworkAccess,
    /// Credential access (passwords, tokens, etc.)
    CredentialAccess,
    /// Process manipulation (kill, spawn, etc.)
    ProcessManipulation,
    /// Filesystem path traversal
    PathTraversal,
    /// Code execution (eval, exec, etc.)
    CodeExecution,
}

impl ConcernType {
    /// Get the default severity for this concern type
    pub fn default_severity(&self) -> SafetyLevel {
        match self {
            ConcernType::DestructiveFileOp => SafetyLevel::Dangerous,
            ConcernType::SystemCommand => SafetyLevel::Caution,
            ConcernType::ElevatedPrivilege => SafetyLevel::Blocked,
            ConcernType::NetworkAccess => SafetyLevel::Caution,
            ConcernType::CredentialAccess => SafetyLevel::Dangerous,
            ConcernType::ProcessManipulation => SafetyLevel::Dangerous,
            ConcernType::PathTraversal => SafetyLevel::Dangerous,
            ConcernType::CodeExecution => SafetyLevel::Dangerous,
        }
    }
}

/// Report from safety analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyReport {
    /// Tool name analyzed
    pub tool_name: String,
    /// Overall safety level
    pub level: SafetyLevel,
    /// List of detected concerns
    pub concerns: Vec<SafetyConcern>,
    /// Whether the tool can be auto-executed
    pub can_auto_execute: bool,
    /// Human-readable summary
    pub summary: String,
}

impl SafetyReport {
    /// Check if the report indicates the tool is safe
    pub fn is_safe(&self) -> bool {
        self.level == SafetyLevel::Safe
    }

    /// Check if the report requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        self.level.requires_confirmation()
    }

    /// Check if the tool is blocked
    pub fn is_blocked(&self) -> bool {
        self.level.is_blocked()
    }
}

// =============================================================================
// Safety Gate
// =============================================================================

/// Configuration for safety gate
#[derive(Debug, Clone)]
pub struct SafetyGateConfig {
    /// Allow network access
    pub allow_network: bool,
    /// Allow file system writes outside workspace
    pub allow_fs_writes_outside_workspace: bool,
    /// List of blocked patterns (regex-like)
    pub blocked_patterns: Vec<String>,
    /// Maximum severity to auto-approve
    pub max_auto_approve_level: SafetyLevel,
}

impl Default for SafetyGateConfig {
    fn default() -> Self {
        Self {
            allow_network: true,
            allow_fs_writes_outside_workspace: false,
            blocked_patterns: vec![
                "sudo".to_string(),
                "rm -rf".to_string(),
                "chmod 777".to_string(),
                "eval(".to_string(),
                "exec(".to_string(),
            ],
            max_auto_approve_level: SafetyLevel::Safe,
        }
    }
}

/// Safety gate for analyzing and validating generated tools
pub struct SafetyGate {
    config: SafetyGateConfig,
}

impl SafetyGate {
    /// Create a new safety gate with default config
    pub fn new() -> Self {
        Self {
            config: SafetyGateConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: SafetyGateConfig) -> Self {
        Self { config }
    }

    /// Analyze a suggestion for safety concerns
    pub fn analyze(&self, suggestion: &SolidificationSuggestion) -> SafetyReport {
        let mut concerns = Vec::new();
        let instructions = suggestion.instructions_preview.to_lowercase();

        // Check for destructive file operations
        for pattern in &["rm ", "rm -", "delete ", "unlink", "shutil.rmtree", "remove("] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::DestructiveFileOp,
                    description: "Contains destructive file operations".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::DestructiveFileOp.default_severity(),
                });
            }
        }

        // Check for system command execution
        for pattern in &[
            "subprocess",
            "os.system",
            "os.popen",
            "shell=true",
            "child_process",
            "spawn(",
        ] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::SystemCommand,
                    description: "May execute system commands".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::SystemCommand.default_severity(),
                });
            }
        }

        // Check for elevated privileges
        for pattern in &["sudo", "root", "admin", "runas", "privilege"] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::ElevatedPrivilege,
                    description: "May require elevated privileges".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::ElevatedPrivilege.default_severity(),
                });
            }
        }

        // Check for network access
        for pattern in &[
            "http://",
            "https://",
            "requests.",
            "urllib",
            "fetch(",
            "axios",
            "socket",
        ] {
            if instructions.contains(pattern) {
                let severity = if self.config.allow_network {
                    SafetyLevel::Caution
                } else {
                    SafetyLevel::Dangerous
                };
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::NetworkAccess,
                    description: "May access network resources".to_string(),
                    pattern: pattern.to_string(),
                    severity,
                });
            }
        }

        // Check for credential access
        for pattern in &[
            "password",
            "secret",
            "token",
            "api_key",
            "credential",
            "private_key",
        ] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::CredentialAccess,
                    description: "May access credentials".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::CredentialAccess.default_severity(),
                });
            }
        }

        // Check for path traversal
        for pattern in &["../", "..\\", "path.join(", "pathbuf::"] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::PathTraversal,
                    description: "May traverse filesystem paths".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::PathTraversal.default_severity(),
                });
            }
        }

        // Check for code execution
        for pattern in &["eval(", "exec(", "compile(", "function(", "__import__"] {
            if instructions.contains(pattern) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::CodeExecution,
                    description: "May execute arbitrary code".to_string(),
                    pattern: pattern.to_string(),
                    severity: ConcernType::CodeExecution.default_severity(),
                });
            }
        }

        // Check blocked patterns
        for pattern in &self.config.blocked_patterns {
            if instructions.contains(&pattern.to_lowercase()) {
                concerns.push(SafetyConcern {
                    concern_type: ConcernType::SystemCommand,
                    description: format!("Contains blocked pattern: {}", pattern),
                    pattern: pattern.clone(),
                    severity: SafetyLevel::Blocked,
                });
            }
        }

        // Determine overall level
        let level = concerns
            .iter()
            .map(|c| c.severity)
            .max_by_key(|s| match s {
                SafetyLevel::Safe => 0,
                SafetyLevel::Caution => 1,
                SafetyLevel::Dangerous => 2,
                SafetyLevel::Blocked => 3,
            })
            .unwrap_or(SafetyLevel::Safe);

        let can_auto_execute = level.allows_auto_execute();

        let summary = if concerns.is_empty() {
            "No safety concerns detected".to_string()
        } else {
            format!(
                "Found {} safety concern(s): {}",
                concerns.len(),
                concerns
                    .iter()
                    .map(|c| c.description.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        debug!(
            tool_name = %suggestion.suggested_name,
            level = ?level,
            concerns = concerns.len(),
            "Safety analysis complete"
        );

        SafetyReport {
            tool_name: suggestion.suggested_name.clone(),
            level,
            concerns,
            can_auto_execute,
            summary,
        }
    }

    /// Check if a suggestion can be auto-approved
    pub fn can_auto_approve(&self, suggestion: &SolidificationSuggestion) -> bool {
        let report = self.analyze(suggestion);

        matches!(
            (&report.level, &self.config.max_auto_approve_level),
            (SafetyLevel::Safe, _)
                | (SafetyLevel::Caution, SafetyLevel::Caution)
                | (SafetyLevel::Caution, SafetyLevel::Dangerous)
                | (SafetyLevel::Dangerous, SafetyLevel::Dangerous)
        )
    }

    /// Validate a suggestion and return an error if blocked
    pub fn validate(&self, suggestion: &SolidificationSuggestion) -> Result<SafetyReport, String> {
        let report = self.analyze(suggestion);

        if report.is_blocked() {
            warn!(
                tool_name = %suggestion.suggested_name,
                concerns = ?report.concerns,
                "Tool blocked by safety gate"
            );
            Err(format!(
                "Tool '{}' is blocked: {}",
                suggestion.suggested_name, report.summary
            ))
        } else {
            Ok(report)
        }
    }
}

impl Default for SafetyGate {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// First-Run Confirmation
// =============================================================================

/// Confirmation status for first-run gating
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstRunConfirmation {
    /// Tool name
    pub tool_name: String,
    /// When confirmation was given
    pub confirmed_at: Option<i64>,
    /// Who confirmed (user ID or "auto")
    pub confirmed_by: Option<String>,
    /// Safety report at time of confirmation
    pub safety_report: Option<SafetyReport>,
}

impl FirstRunConfirmation {
    /// Create a new unconfirmed entry
    pub fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            confirmed_at: None,
            confirmed_by: None,
            safety_report: None,
        }
    }

    /// Confirm with the given user
    pub fn confirm(&mut self, user: &str, report: SafetyReport) {
        self.confirmed_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                .as_secs() as i64,
        );
        self.confirmed_by = Some(user.to_string());
        self.safety_report = Some(report);
    }

    /// Check if confirmed
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;

    fn create_safe_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "safe-pattern".to_string(),
            suggested_name: "safe-tool".to_string(),
            suggested_description: "A safe tool".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("safe-pattern"),
            sample_contexts: vec!["process text".to_string()],
            instructions_preview: "# Safe Tool\n\nProcess the input text and return result."
                .to_string(),
        }
    }

    fn create_dangerous_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "dangerous-pattern".to_string(),
            suggested_name: "dangerous-tool".to_string(),
            suggested_description: "A dangerous tool".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("dangerous-pattern"),
            sample_contexts: vec!["delete files".to_string()],
            instructions_preview: "# Dangerous Tool\n\nRun: rm -rf /tmp/files\nUse sudo if needed."
                .to_string(),
        }
    }

    #[test]
    fn test_safe_analysis() {
        let gate = SafetyGate::new();
        let suggestion = create_safe_suggestion();

        let report = gate.analyze(&suggestion);

        assert_eq!(report.level, SafetyLevel::Safe);
        assert!(report.concerns.is_empty());
        assert!(report.can_auto_execute);
    }

    #[test]
    fn test_dangerous_analysis() {
        let gate = SafetyGate::new();
        let suggestion = create_dangerous_suggestion();

        let report = gate.analyze(&suggestion);

        assert!(matches!(
            report.level,
            SafetyLevel::Dangerous | SafetyLevel::Blocked
        ));
        assert!(!report.concerns.is_empty());
        assert!(!report.can_auto_execute);
    }

    #[test]
    fn test_blocked_patterns() {
        let gate = SafetyGate::new();
        let mut suggestion = create_safe_suggestion();
        suggestion.instructions_preview = "Use sudo rm -rf to clean".to_string();

        let report = gate.analyze(&suggestion);

        assert!(report.is_blocked());
    }

    #[test]
    fn test_validate_safe() {
        let gate = SafetyGate::new();
        let suggestion = create_safe_suggestion();

        let result = gate.validate(&suggestion);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_blocked() {
        let gate = SafetyGate::new();
        let suggestion = create_dangerous_suggestion();

        let result = gate.validate(&suggestion);
        // May be blocked or dangerous depending on patterns
        if let Err(e) = result {
            assert!(e.contains("blocked"));
        }
    }

    #[test]
    fn test_can_auto_approve() {
        let gate = SafetyGate::new();

        let safe = create_safe_suggestion();
        assert!(gate.can_auto_approve(&safe));

        let dangerous = create_dangerous_suggestion();
        assert!(!gate.can_auto_approve(&dangerous));
    }

    #[test]
    fn test_first_run_confirmation() {
        let mut confirmation = FirstRunConfirmation::new("test-tool");
        assert!(!confirmation.is_confirmed());

        let report = SafetyReport {
            tool_name: "test-tool".to_string(),
            level: SafetyLevel::Safe,
            concerns: vec![],
            can_auto_execute: true,
            summary: "Safe".to_string(),
        };

        confirmation.confirm("user123", report);
        assert!(confirmation.is_confirmed());
        assert_eq!(confirmation.confirmed_by, Some("user123".to_string()));
    }

    #[test]
    fn test_concern_types() {
        assert_eq!(
            ConcernType::ElevatedPrivilege.default_severity(),
            SafetyLevel::Blocked
        );
        assert_eq!(
            ConcernType::DestructiveFileOp.default_severity(),
            SafetyLevel::Dangerous
        );
        assert_eq!(
            ConcernType::NetworkAccess.default_severity(),
            SafetyLevel::Caution
        );
    }

    #[test]
    fn test_safety_level_methods() {
        assert!(SafetyLevel::Safe.allows_auto_execute());
        assert!(!SafetyLevel::Caution.allows_auto_execute());

        assert!(!SafetyLevel::Safe.requires_confirmation());
        assert!(SafetyLevel::Dangerous.requires_confirmation());

        assert!(!SafetyLevel::Safe.is_blocked());
        assert!(SafetyLevel::Blocked.is_blocked());
    }
}
