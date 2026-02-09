//! High-level sandbox execution orchestration
//!
//! Provides SandboxManager for coordinating sandbox execution with automatic
//! profile generation, cleanup, and audit logging.

use crate::error::{AlephError, Result};
use crate::exec::sandbox::adapter::{ExecutionResult, SandboxAdapter, SandboxCommand};
use crate::exec::sandbox::audit::{ExecutionStatus, SandboxAuditLog};
use crate::exec::sandbox::capabilities::Capabilities;
use std::sync::Arc;

/// Policy for handling sandbox unavailability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPolicy {
    /// Deny execution if sandbox is unavailable
    Deny,
    /// Request user approval before executing without sandbox
    RequestApproval,
    /// Warn user but execute without sandbox
    WarnAndExecute,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self::Deny
    }
}

/// High-level sandbox execution manager
///
/// Orchestrates sandbox execution with automatic profile generation,
/// cleanup, and audit logging.
pub struct SandboxManager {
    adapter: Arc<dyn SandboxAdapter>,
    fallback_policy: FallbackPolicy,
}

impl SandboxManager {
    /// Create a new sandbox manager with default fallback policy (Deny)
    pub fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            adapter,
            fallback_policy: FallbackPolicy::default(),
        }
    }

    /// Create a sandbox manager with custom fallback policy
    pub fn with_fallback_policy(
        adapter: Arc<dyn SandboxAdapter>,
        fallback_policy: FallbackPolicy,
    ) -> Self {
        Self {
            adapter,
            fallback_policy,
        }
    }

    /// Check if sandbox is available on current platform
    pub fn is_available(&self) -> bool {
        self.adapter.is_supported()
    }

    /// Execute command in sandbox with automatic profile management
    ///
    /// Returns both the execution result and an audit log.
    /// Automatically generates profile, executes command, and cleans up.
    pub async fn execute_sandboxed(
        &self,
        skill_id: &str,
        command: &SandboxCommand,
        capabilities: &Capabilities,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        // Check if sandbox is available
        if !self.is_available() {
            return self.handle_sandbox_unavailable(skill_id, command, capabilities).await;
        }

        // Generate sandbox profile
        let profile = self.adapter.generate_profile(capabilities)?;

        // Execute command in sandbox
        let result = self.adapter.execute_sandboxed(command, &profile).await;

        // Create audit log
        let execution_status = match &result {
            Ok(exec_result) => {
                if let Some(exit_code) = exec_result.exit_code {
                    ExecutionStatus::Success {
                        exit_code,
                        duration_ms: exec_result.duration_ms,
                    }
                } else {
                    ExecutionStatus::Error {
                        error: "Process terminated without exit code".to_string(),
                    }
                }
            }
            Err(AlephError::ExecutionTimeout { timeout_secs }) => ExecutionStatus::Timeout {
                duration_ms: timeout_secs * 1000,
            },
            Err(e) => ExecutionStatus::Error {
                error: e.to_string(),
            },
        };

        let audit_log = SandboxAuditLog::new(
            skill_id.to_string(),
            capabilities.clone(),
            execution_status,
            self.adapter.platform_name().to_string(),
        );

        // Cleanup profile (even if execution failed)
        let _ = self.adapter.cleanup(&profile);

        // Return result and audit log
        result.map(|r| (r, audit_log))
    }

    /// Handle sandbox unavailability based on fallback policy
    async fn handle_sandbox_unavailable(
        &self,
        skill_id: &str,
        _command: &SandboxCommand,
        capabilities: &Capabilities,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        let reason = format!(
            "Sandbox not supported on platform: {}",
            self.adapter.platform_name()
        );

        match self.fallback_policy {
            FallbackPolicy::Deny => {
                let _audit_log = SandboxAuditLog::new(
                    skill_id.to_string(),
                    capabilities.clone(),
                    ExecutionStatus::Error {
                        error: reason.clone(),
                    },
                    self.adapter.platform_name().to_string(),
                );
                Err(AlephError::SandboxUnavailable { reason })
            }
            FallbackPolicy::RequestApproval => {
                // TODO: Implement approval workflow
                let _audit_log = SandboxAuditLog::new(
                    skill_id.to_string(),
                    capabilities.clone(),
                    ExecutionStatus::Error {
                        error: "Approval workflow not implemented".to_string(),
                    },
                    self.adapter.platform_name().to_string(),
                );
                Err(AlephError::SandboxUnavailable {
                    reason: "Approval workflow not implemented".to_string(),
                })
            }
            FallbackPolicy::WarnAndExecute => {
                // TODO: Implement unsandboxed execution with warning
                let _audit_log = SandboxAuditLog::new(
                    skill_id.to_string(),
                    capabilities.clone(),
                    ExecutionStatus::Error {
                        error: "Unsandboxed execution not implemented".to_string(),
                    },
                    self.adapter.platform_name().to_string(),
                );
                Err(AlephError::SandboxUnavailable {
                    reason: "Unsandboxed execution not implemented".to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::platforms::macos::MacOSSandbox;

    #[tokio::test]
    async fn test_sandbox_manager_execution() {
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = SandboxManager::new(adapter);

        if !manager.is_available() {
            println!("Skipping test: sandbox not available");
            return;
        }

        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["test".to_string()],
            working_dir: None,
        };

        let caps = Capabilities::default();
        let (result, audit_log) = manager
            .execute_sandboxed("test-skill", &command, &caps)
            .await
            .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("test"));
        assert_eq!(audit_log.skill_id, "test-skill");
    }
}
