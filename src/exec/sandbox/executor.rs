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
    Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability, ProcessCapability,
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
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> std::result::Result<SandboxOutput, SandboxError> {
        use crate::exec::sandbox::adapter::SandboxProfile as LegacySandboxProfile;

        // stdin piping isn't natively supported by the legacy
        // `execute_sandboxed` seam (it uses `Command::output`, which does
        // not accept stdin bytes). Phase 3 follow-up: thread stdin through
        // the adapter. For now, warn if the caller passed stdin so bugs
        // don't hide behind silent drops.
        if stdin.is_some() {
            tracing::warn!(
                program = %program,
                "OsSandboxDriver::run received stdin bytes but legacy adapter does \
                 not yet support stdin piping — bytes are ignored"
            );
        }

        // Build a legacy profile where `max_execution_time` reflects the
        // caller-supplied timeout. The adapter's internal `tokio::time::timeout`
        // consults this field, so this is how the configured SandboxConfig
        // timeout flows into sandbox-exec without a new API.
        let profile_path = crate::exec::sandbox::profile::ProfileGenerator::write_temp_profile(
            &profile.contents,
            ".sb",
        )
        .map_err(|e| SandboxError::ProfileGeneration(e.to_string()))?;

        // Clamp timeout to u64 seconds, rounding up so a 500ms caller does not
        // quietly become 0s ("run forever"). Minimum of 1s enforced because
        // `ExecutionTimeout` carries a `timeout_secs: u64`.
        let timeout_secs = timeout
            .as_secs()
            .saturating_add(if timeout.subsec_nanos() > 0 { 1 } else { 0 })
            .max(1);

        let mut legacy_caps = Capabilities::default();
        legacy_caps.process.max_execution_time = timeout_secs;

        let legacy_profile = LegacySandboxProfile {
            path: profile_path.clone(),
            capabilities: legacy_caps,
            platform: self.adapter.platform_name().to_string(),
            temp_workspace: None,
        };

        let command = SandboxCommand {
            program: program.to_string(),
            args: args.to_vec(),
            working_dir: Some(PathBuf::from(cwd)),
            env: env.clone(),
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
            Ok(result) => {
                // Clamp stdout / stderr to `max_output_bytes` per stream.
                // Streams are truncated independently; `truncated` flips if
                // either exceeded the cap. We truncate on UTF-8 char boundaries
                // to avoid producing invalid strings.
                let (stdout_bytes, stdout_truncated) =
                    truncate_utf8(result.stdout, max_output_bytes);
                let (stderr_bytes, stderr_truncated) =
                    truncate_utf8(result.stderr, max_output_bytes);
                let truncated = stdout_truncated || stderr_truncated;
                if truncated {
                    tracing::warn!(
                        program = %program,
                        max_output_bytes,
                        "Sandbox output truncated to fit max_output_bytes cap"
                    );
                }
                Ok(SandboxOutput {
                    stdout: stdout_bytes,
                    stderr: stderr_bytes,
                    exit_code: result.exit_code,
                    signal: None,
                    truncated,
                    duration_ms: result.duration_ms,
                })
            }
            Err(err) => {
                let _ = start.elapsed();
                Err(map_exec_error(err))
            }
        }
    }
}

/// Truncate a string to at most `max_bytes`, returning the resulting bytes
/// plus a flag indicating whether truncation occurred. Truncates on a UTF-8
/// char boundary so downstream `String::from_utf8_lossy` callers do not see
/// a split code point.
fn truncate_utf8(s: String, max_bytes: usize) -> (Vec<u8>, bool) {
    if s.len() <= max_bytes {
        return (s.into_bytes(), false);
    }
    // Walk back to the nearest char boundary <= max_bytes.
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut bytes = s.into_bytes();
    bytes.truncate(cut);
    (bytes, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
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
            env: HashMap::new(),
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

    /// H5 regression: `OsSandboxDriver::run` must pass `env` through to the
    /// subprocess. Prior to the fix, the field was bound with `_env` and
    /// silently dropped, so CodeExecTool's carefully-built PATH injection
    /// never reached the sandboxed child.
    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn run_honors_env() {
        use tempfile::tempdir;

        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let driver = OsSandboxDriver::new(adapter.clone());
        if !driver.is_available() {
            println!("Skipping run_honors_env: sandbox-exec unavailable");
            return;
        }

        let caps = NewSandboxCapabilities::strict();
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let profile = driver.profile_for(&caps, cwd).expect("profile");

        let mut env = HashMap::new();
        env.insert("OMCTEST_FOO".to_string(), "bar".to_string());
        let output = driver
            .run(
                "sh",
                &["-c".to_string(), "printf %s \"$OMCTEST_FOO\"".to_string()],
                &env,
                None,
                cwd,
                &profile,
                Duration::from_secs(5),
                1024,
            )
            .await
            .expect("run ok");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert_eq!(output.exit_code, Some(0));
        assert!(
            stdout.contains("bar"),
            "expected env FOO=bar to reach subprocess, got stdout={stdout:?}"
        );
    }

    /// H5 regression: `run` must enforce the caller-supplied timeout. Prior
    /// to the fix, `_timeout` was ignored and the legacy 300s default was
    /// used, so per-command budgets never applied.
    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn run_honors_timeout() {
        use tempfile::tempdir;

        let adapter: Arc<dyn SandboxAdapter> = Arc::new(MacOSSandbox::new());
        let driver = OsSandboxDriver::new(adapter);
        if !driver.is_available() {
            println!("Skipping run_honors_timeout: sandbox-exec unavailable");
            return;
        }

        let caps = NewSandboxCapabilities::strict();
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let profile = driver.profile_for(&caps, cwd).expect("profile");

        // `sleep 5` with a 1s timeout must return `SandboxError::Timeout`.
        // We sub-second round-up ensures `Duration::from_millis(500)` does
        // not become 0s (which would disable the timeout entirely).
        let res = driver
            .run(
                "sleep",
                &["5".to_string()],
                &HashMap::new(),
                None,
                cwd,
                &profile,
                Duration::from_millis(500),
                1024,
            )
            .await;
        match res {
            Err(SandboxError::Timeout { .. }) => {}
            other => panic!("expected SandboxError::Timeout, got {other:?}"),
        }
    }

    #[test]
    fn truncate_utf8_preserves_short_strings() {
        let (bytes, truncated) = truncate_utf8("hello".to_string(), 100);
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_utf8_clamps_ascii() {
        let (bytes, truncated) = truncate_utf8("abcdef".to_string(), 3);
        assert_eq!(bytes, b"abc");
        assert!(truncated);
    }

    #[test]
    fn truncate_utf8_respects_char_boundary() {
        // "é" is 2 bytes (0xC3 0xA9). Cutting at 1 byte must walk back to 0.
        let (bytes, truncated) = truncate_utf8("é".to_string(), 1);
        assert!(truncated);
        // Walked back to 0 to avoid splitting the code point.
        assert!(bytes.is_empty());
    }
}
