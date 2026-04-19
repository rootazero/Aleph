//! OsSandboxDriver — OS-level sandbox-exec profile driver (macOS).
//!
//! Provides the OS-level sandbox execution orchestration consumed by
//! `WorkspaceSandbox` in `src/sandbox/`. Owns profile generation, subprocess
//! spawning, cleanup, and audit logging.
//!
//! Previously named `SandboxManager`; renamed in Phase 3 Task 4 so the
//! "OS driver" role is explicit relative to the higher-level `Sandbox` trait
//! in `src/sandbox/mod.rs`. Do NOT confuse this with that agent-level trait.

use crate::error::{AlephError, Result};
use crate::exec::sandbox::adapter::{ExecutionResult, SandboxAdapter, SandboxCommand};
use crate::exec::sandbox::audit::{ExecutionStatus, SandboxAuditLog};
use crate::exec::sandbox::capabilities::{
    Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability,
    ProcessCapability,
};
use crate::sandbox::capabilities::{
    NetworkPolicy as NewNetworkPolicy, SandboxCapabilities as NewSandboxCapabilities,
};
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Policy for handling sandbox unavailability
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// Deny execution if sandbox is unavailable
    #[default]
    Deny,
    /// Request user approval before executing without sandbox
    RequestApproval,
    /// Warn user but execute without sandbox
    WarnAndExecute,
}

/// OS-level sandbox driver for exec-class tools.
///
/// Orchestrates sandbox execution with automatic profile generation,
/// cleanup, and audit logging. Implements `OsSandboxDriverTrait` so the
/// higher-level `WorkspaceSandbox` can drive it through a stable seam.
pub struct OsSandboxDriver {
    adapter: Arc<dyn SandboxAdapter>,
    fallback_policy: FallbackPolicy,
}

impl OsSandboxDriver {
    /// Create a new OS sandbox driver with default fallback policy (Deny)
    pub fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            adapter,
            fallback_policy: FallbackPolicy::default(),
        }
    }

    /// Create with custom fallback policy
    pub fn with_fallback_policy(mut self, policy: FallbackPolicy) -> Self {
        self.fallback_policy = policy;
        self
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
        command: SandboxCommand,
        capabilities: Capabilities,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        // Check if sandbox is available
        if !self.is_available() {
            return self.handle_sandbox_unavailable(skill_id).await;
        }

        // Generate sandbox profile
        let profile = self.adapter.generate_profile(&capabilities)?;

        // Execute command in sandbox
        let result = self.adapter.execute_sandboxed(&command, &profile).await;

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
                duration_ms: timeout_secs.saturating_mul(1000),
            },
            Err(e) => ExecutionStatus::Error {
                error: e.to_string(),
            },
        };

        let audit_log = SandboxAuditLog::new(
            skill_id.to_string(),
            capabilities,
            execution_status,
            self.adapter.platform_name().to_string(),
        );

        // Cleanup profile (even if execution failed).
        // Log cleanup errors but don't let them override the execution result.
        if let Err(e) = self.adapter.cleanup(&profile) {
            tracing::warn!("Sandbox cleanup failed: {}", e);
        }

        // Return result and audit log
        result.map(|r| (r, audit_log))
    }

    /// Handle sandbox unavailability based on fallback policy
    async fn handle_sandbox_unavailable(
        &self,
        _skill_id: &str,
    ) -> Result<(ExecutionResult, SandboxAuditLog)> {
        let reason = format!(
            "Sandbox not supported on platform: {}",
            self.adapter.platform_name()
        );

        match self.fallback_policy {
            FallbackPolicy::Deny => Err(AlephError::SandboxUnavailable { reason }),
            FallbackPolicy::RequestApproval => {
                // TODO: Implement approval workflow
                Err(AlephError::SandboxUnavailable {
                    reason: "Approval workflow not implemented".to_string(),
                })
            }
            FallbackPolicy::WarnAndExecute => {
                // TODO: Implement unsandboxed execution with warning
                Err(AlephError::SandboxUnavailable {
                    reason: "Unsandboxed execution not implemented".to_string(),
                })
            }
        }
    }
}

/// Bridge the higher-level `SandboxCapabilities` (`src/sandbox/capabilities.rs`)
/// into the legacy `Capabilities` shape consumed by the existing adapter.
///
/// Preserves behavior: filesystem paths become `ReadWrite`, network policy is
/// mapped entry-for-entry, and process/environment settings fall back to
/// `Capabilities::default()`.
fn bridge_capabilities(caps: &NewSandboxCapabilities, cwd: &Path) -> Capabilities {
    let mut filesystem: Vec<FileSystemCapability> = Vec::new();
    for path in &caps.fs_read {
        filesystem.push(FileSystemCapability::ReadOnly { path: path.clone() });
    }
    for path in &caps.fs_write {
        filesystem.push(FileSystemCapability::ReadWrite { path: path.clone() });
    }
    if filesystem.is_empty() {
        // Fall back to workspace cwd as the default writable area.
        filesystem.push(FileSystemCapability::ReadWrite {
            path: cwd.to_path_buf(),
        });
    }

    let network = match &caps.network {
        NewNetworkPolicy::None => NetworkCapability::Deny,
        NewNetworkPolicy::AllowAll => NetworkCapability::AllowAll,
        NewNetworkPolicy::AllowHosts { hosts } => NetworkCapability::AllowDomains(hosts.clone()),
    };

    Capabilities {
        filesystem,
        network,
        process: ProcessCapability {
            no_fork: !caps.spawn_subprocess,
            max_execution_time: 300,
            max_memory_mb: Some(512),
        },
        environment: EnvironmentCapability::Restricted,
    }
}

fn map_exec_error(err: AlephError) -> SandboxError {
    match err {
        AlephError::ExecutionTimeout { timeout_secs } => SandboxError::Timeout {
            elapsed_ms: timeout_secs.saturating_mul(1000),
        },
        AlephError::SandboxUnavailable { reason } => SandboxError::CapabilityDenied { reason },
        other => SandboxError::Other(other.to_string()),
    }
}

#[async_trait]
impl OsSandboxDriverTrait for OsSandboxDriver {
    fn profile_for(
        &self,
        capabilities: &NewSandboxCapabilities,
        cwd: &Path,
    ) -> std::result::Result<OsSandboxProfile, SandboxError> {
        let legacy_caps = bridge_capabilities(capabilities, cwd);
        let generated = self
            .adapter
            .generate_profile(&legacy_caps)
            .map_err(|e| SandboxError::ProfileGeneration(e.to_string()))?;
        let contents = std::fs::read_to_string(&generated.path)
            .map_err(|e| SandboxError::ProfileGeneration(e.to_string()))?;
        // Clean up the scratch profile file now that we've captured its bytes.
        if let Err(e) = self.adapter.cleanup(&generated) {
            tracing::warn!("Sandbox profile cleanup failed: {}", e);
        }
        Ok(OsSandboxProfile { contents })
    }

    async fn run(
        &self,
        program: &str,
        args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> std::result::Result<SandboxOutput, SandboxError> {
        use crate::exec::sandbox::adapter::SandboxProfile as LegacySandboxProfile;

        // Persist the profile bytes back to a temp file so the adapter can
        // pass `-f <path>` to sandbox-exec.
        let profile_path = crate::exec::sandbox::profile::ProfileGenerator::write_temp_profile(
            &profile.contents,
            ".sb",
        )
        .map_err(|e| SandboxError::ProfileGeneration(e.to_string()))?;

        let legacy_profile = LegacySandboxProfile {
            path: profile_path.clone(),
            capabilities: Capabilities::default(),
            platform: self.adapter.platform_name().to_string(),
            temp_workspace: None,
        };

        let command = SandboxCommand {
            program: program.to_string(),
            args: args.to_vec(),
            working_dir: Some(PathBuf::from(cwd)),
        };

        let start = Instant::now();
        let exec_result = self
            .adapter
            .execute_sandboxed(&command, &legacy_profile)
            .await;

        if let Err(e) = self.adapter.cleanup(&legacy_profile) {
            tracing::warn!("Sandbox profile cleanup failed: {}", e);
        }

        match exec_result {
            Ok(result) => Ok(SandboxOutput {
                stdout: result.stdout.into_bytes(),
                stderr: result.stderr.into_bytes(),
                exit_code: result.exit_code,
                signal: None,
                truncated: false,
                duration_ms: result.duration_ms,
            }),
            Err(err) => {
                let _ = start.elapsed();
                Err(map_exec_error(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::platforms::macos::MacOSSandbox;

    #[test]
    fn test_fallback_policy_default() {
        let policy = FallbackPolicy::default();
        assert!(matches!(policy, FallbackPolicy::Deny));
    }

    #[tokio::test]
    async fn test_os_sandbox_driver_execution() {
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let manager = OsSandboxDriver::new(adapter);

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
            .execute_sandboxed("test-skill", command, caps)
            .await
            .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("test"));
        assert_eq!(audit_log.skill_id, "test-skill");
    }
}
