// Audit logging for sandbox execution

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Execution status of a sandboxed command
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    /// Command completed successfully
    Success,
    /// Command exceeded time limit
    Timeout,
    /// Command violated sandbox policy
    SandboxViolation,
    /// Command failed with error
    Error(String),
}

/// Type of sandbox violation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationType {
    /// Attempted unauthorized network access
    NetworkAccess,
    /// Attempted unauthorized file system access
    FileSystemAccess,
    /// Attempted unauthorized process execution
    ProcessExecution,
    /// Attempted unauthorized environment variable access
    EnvironmentAccess,
}

/// Record of a sandbox policy violation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    /// Type of violation
    pub violation_type: ViolationType,
    /// Description of attempted action
    pub attempted_action: String,
    /// When the violation occurred
    pub timestamp: DateTime<Utc>,
}

/// Complete audit log for a sandbox execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxAuditLog {
    /// Skill identifier
    pub skill_id: String,
    /// Command that was executed
    pub command: Vec<String>,
    /// When execution started
    pub start_time: DateTime<Utc>,
    /// When execution ended
    pub end_time: Option<DateTime<Utc>>,
    /// Execution duration
    pub duration: Option<Duration>,
    /// Final execution status
    pub status: ExecutionStatus,
    /// List of policy violations
    pub violations: Vec<SandboxViolation>,
    /// Standard output (truncated if too large)
    pub stdout: Option<String>,
    /// Standard error (truncated if too large)
    pub stderr: Option<String>,
    /// Exit code if available
    pub exit_code: Option<i32>,
}

impl SandboxAuditLog {
    /// Create a new audit log for a command execution
    pub fn new(skill_id: String, command: Vec<String>) -> Self {
        Self {
            skill_id,
            command,
            start_time: Utc::now(),
            end_time: None,
            duration: None,
            status: ExecutionStatus::Success,
            violations: Vec::new(),
            stdout: None,
            stderr: None,
            exit_code: None,
        }
    }

    /// Add a violation to the audit log
    pub fn add_violation(&mut self, violation: SandboxViolation) {
        self.violations.push(violation);
        self.status = ExecutionStatus::SandboxViolation;
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        matches!(self.status, ExecutionStatus::Success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_creation() {
        let log = SandboxAuditLog::new(
            "skill-test".to_string(),
            vec!["echo".to_string(), "hello".to_string()],
        );

        assert_eq!(log.skill_id, "skill-test");
        assert_eq!(log.command, vec!["echo", "hello"]);
        assert!(log.violations.is_empty());
        assert!(log.is_success());
    }

    #[test]
    fn test_audit_log_with_violation() {
        let mut log = SandboxAuditLog::new(
            "skill-test".to_string(),
            vec!["curl".to_string(), "evil.com".to_string()],
        );

        log.add_violation(SandboxViolation {
            violation_type: ViolationType::NetworkAccess,
            attempted_action: "connect to evil.com".to_string(),
            timestamp: Utc::now(),
        });

        assert_eq!(log.violations.len(), 1);
        assert!(!log.is_success());
    }
}
