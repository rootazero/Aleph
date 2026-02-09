// Audit logging for sandbox execution

use crate::exec::sandbox::capabilities::Capabilities;
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Execution status of a sandboxed command
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Command completed successfully
    Success { exit_code: i32, duration_ms: u64 },
    /// Command exceeded time limit
    Timeout { duration_ms: u64 },
    /// Command violated sandbox policy
    SandboxViolation { violation: String },
    /// Command failed with error
    Error { error: String },
}

/// Type of sandbox violation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationType {
    /// Attempted unauthorized file access
    UnauthorizedFileAccess,
    /// Attempted unauthorized network access
    UnauthorizedNetworkAccess,
    /// Attempted unauthorized process fork
    UnauthorizedProcessFork,
    /// Resource limit exceeded
    ResourceLimitExceeded,
}

/// Record of a sandbox policy violation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    /// Type of violation
    pub violation_type: ViolationType,
    /// Description of the violation
    pub description: String,
    /// When the violation occurred (Unix timestamp)
    pub timestamp: i64,
}

/// Complete audit log for a sandbox execution
#[derive(Debug, Serialize)]
pub struct SandboxAuditLog {
    /// When execution started (Unix timestamp)
    pub timestamp: i64,
    /// Skill identifier
    pub skill_id: String,
    /// Sandbox capabilities used
    pub capabilities: Capabilities,
    /// Final execution status
    pub execution_result: ExecutionStatus,
    /// Platform used for sandboxing
    pub sandbox_platform: String,
    /// List of policy violations
    pub violations: Vec<SandboxViolation>,
}

impl SandboxAuditLog {
    /// Create a new audit log for a sandbox execution
    pub fn new(
        skill_id: String,
        capabilities: Capabilities,
        execution_result: ExecutionStatus,
        sandbox_platform: String,
    ) -> Self {
        Self {
            timestamp: Utc::now().timestamp(),
            skill_id,
            capabilities,
            execution_result,
            sandbox_platform,
            violations: Vec::new(),
        }
    }

    /// Add a violation to the audit log
    pub fn add_violation(&mut self, violation: SandboxViolation) {
        self.violations.push(violation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_serialization() {
        let log = SandboxAuditLog::new(
            "test-skill".to_string(),
            Capabilities::default(),
            ExecutionStatus::Success {
                exit_code: 0,
                duration_ms: 100,
            },
            "macos".to_string(),
        );

        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("test-skill"));
        assert!(json.contains("\"status\":\"success\""));
    }
}
