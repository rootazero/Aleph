//! macOS Seatbelt sandbox driver — generates SBPL profiles and executes
//! via `/usr/bin/sandbox-exec`.
//!
//! Inspired by codex's seatbelt implementation but adapted for Aleph's
//! SandboxPolicy / SandboxCapabilities model.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::debug;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::policy::{EnvPolicy, FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

/// Path to the trusted `sandbox-exec` binary.
/// We only trust `/usr/bin/sandbox-exec` to defend against PATH injection.
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Base SBPL policy — closed-by-default with essential macOS allowances.
const BASE_POLICY: &str = r#"(version 1)

; closed-by-default
(deny default)

; child processes inherit parent's policy
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))

; essential device access
(allow file-write-data
  (require-all
    (path "/dev/null")
    (vnode-type CHARACTER-DEVICE)))

; common sysctls for CPU detection, memory info, etc.
(allow sysctl-read
  (sysctl-name "hw.activecpu")
  (sysctl-name "hw.byteorder")
  (sysctl-name "hw.cpufamily")
  (sysctl-name "hw.cputype")
  (sysctl-name "hw.logicalcpu")
  (sysctl-name "hw.logicalcpu_max")
  (sysctl-name "hw.machine")
  (sysctl-name "hw.memsize")
  (sysctl-name "hw.ncpu")
  (sysctl-name "hw.pagesize")
  (sysctl-name "hw.physicalcpu")
  (sysctl-name "hw.physicalcpu_max")
  (sysctl-name-prefix "hw.optional.arm.")
  (sysctl-name "kern.hostname")
  (sysctl-name "kern.osproductversion")
  (sysctl-name "kern.osrelease")
  (sysctl-name "kern.ostype")
  (sysctl-name "kern.osversion")
  (sysctl-name "vm.loadavg"))

; IOKit for power management
(allow iokit-open (iokit-registry-entry-class "RootDomainUserClient"))

; Directory services
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))

; POSIX semaphores (Python multiprocessing)
(allow ipc-posix-sem)

; Power management
(allow mach-lookup (global-name "com.apple.PowerManagement.control"))

; PTY support
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write*
  (require-all
    (regex #"^/dev/ttys[0-9]+")
    (extension "com.apple.sandbox.pty")))
(allow file-ioctl (regex #"^/dev/ttys[0-9]+"))

; CoreFoundation preferences
(allow ipc-posix-shm-read* (ipc-posix-name-prefix "apple.cfprefs."))
(allow mach-lookup
  (global-name "com.apple.cfprefsd.daemon")
  (global-name "com.apple.cfprefsd.agent")
  (local-name "com.apple.cfprefsd.agent"))
(allow user-preference-read)
"#;

/// Network policy template for restricted network access.
const RESTRICTED_NETWORK_POLICY: &str = r#"
; deny all network by default
(deny network*)

; allow localhost DNS
(allow network-outbound (remote ip "localhost:53"))
"#;

/// Driver for macOS seatbelt sandboxing.
#[derive(Debug, Clone)]
pub struct SeatbeltDriver;

impl SeatbeltDriver {
    pub fn new() -> Self {
        Self
    }

    /// Check if `sandbox-exec` is available and executable.
    fn check_sandbox_exec(&self) -> bool {
        std::fs::metadata(SANDBOX_EXEC_PATH)
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    /// Generate SBPL profile from SandboxPolicy.
    fn generate_profile(&self, policy: &SandboxPolicy, cwd: &Path) -> Result<String, SandboxError> {
        let mut profile = String::with_capacity(4096);

        // Base policy
        profile.push_str(BASE_POLICY);
        profile.push('\n');

        // Filesystem policy
        self.add_fs_policy(&mut profile, &policy.filesystem, cwd)?;

        // Network policy
        self.add_network_policy(&mut profile, &policy.network);

        // Process policy
        self.add_process_policy(&mut profile, &policy.process);

        // Environment policy
        self.add_env_policy(&mut profile, &policy.environment);

        debug!("generated seatbelt profile ({} bytes)", profile.len());
        Ok(profile)
    }

    fn add_fs_policy(
        &self,
        profile: &mut String,
        fs: &FsPolicy,
        cwd: &Path,
    ) -> Result<(), SandboxError> {
        let cwd_str = cwd.to_str().ok_or_else(|| {
            SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
        })?;

        match fs {
            FsPolicy::WorkspaceOnly => {
                profile.push_str(&format!(
                    "; workspace-only filesystem access\n\
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
                ));
            }
            FsPolicy::ReadPaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
                ));
                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    profile.push_str(&format!(
                        "(allow file-read* (subpath \"{}\"))\n",
                        path_str
                    ));
                }
            }
            FsPolicy::WritePaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
                ));
                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        path_str
                    ));
                }
            }
            FsPolicy::FullRead { exclude } => {
                profile.push_str("; full read access\n(allow file-read*)\n");
                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    profile.push_str(&format!(
                        "(deny file-read* (subpath \"{}\"))\n",
                        path_str
                    ));
                }
            }
            FsPolicy::FullWrite { exclude } => {
                profile.push_str(
                    "; full read/write access\n(allow file-read* file-write*)\n",
                );
                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    profile.push_str(&format!(
                        "(deny file-read* file-write* (subpath \"{}\"))\n",
                        path_str
                    ));
                }
            }
        }
        Ok(())
    }

    fn add_network_policy(&self, profile: &mut String, network: &NetworkPolicy) {
        match network {
            NetworkPolicy::None => {
                profile.push_str("; no network access\n(deny network*)\n");
            }
            NetworkPolicy::AllowAll => {
                profile.push_str("; full network access\n(allow network*)\n");
            }
            NetworkPolicy::AllowHosts(hosts) => {
                profile.push_str(RESTRICTED_NETWORK_POLICY);
                for host in hosts {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote ip \"{}\"))\n",
                        host
                    ));
                }
            }
            NetworkPolicy::ProxyOnly { ports } => {
                profile.push_str(RESTRICTED_NETWORK_POLICY);
                profile.push_str("; proxy-only network access\n");
                for port in ports {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote ip \"localhost:{}\"))\n",
                        port
                    ));
                }
            }
        }
    }

    fn add_process_policy(&self, profile: &mut String, process: &ProcessPolicy) {
        if !process.allow_fork {
            profile.push_str("; deny subprocess spawning\n(deny process-fork)\n");
        }
    }

    fn add_env_policy(&self, profile: &mut String, env: &EnvPolicy) {
        match env {
            EnvPolicy::Inherit => {
                // Default — no restrictions
            }
            EnvPolicy::Restricted => {
                profile.push_str(
                    "; restricted environment\n(allow process-exec (with environment))\n",
                );
            }
            EnvPolicy::Minimal => {
                profile.push_str(
                    "; minimal environment\n(allow process-exec (with environment))\n",
                );
            }
        }
    }
}

#[async_trait]
impl OsSandboxDriverTrait for SeatbeltDriver {
    fn platform(&self) -> &'static str {
        "macos/seatbelt"
    }

    fn is_supported(&self) -> bool {
        self.check_sandbox_exec()
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        let policy = SandboxPolicy::from(capabilities);
        let contents = self.generate_profile(&policy, cwd)?;
        Ok(OsSandboxProfile { contents })
    }

    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<SandboxOutput, SandboxError> {
        if !self.is_supported() {
            return Err(SandboxError::ExecutionFailed(
                "sandbox-exec not available".into(),
            ));
        }

        // Write profile to a temporary file
        let profile_file = tempfile::NamedTempFile::new().map_err(|e| {
            SandboxError::Io(format!("failed to create temp file for profile: {e}"))
        })?;
        std::fs::write(profile_file.path(), &profile.contents).map_err(|e| {
            SandboxError::Io(format!("failed to write profile: {e}"))
        })?;

        debug!(
            "running sandbox-exec with profile ({} bytes)",
            profile.contents.len()
        );

        let mut cmd = Command::new(SANDBOX_EXEC_PATH);
        cmd.arg("-f")
            .arg(profile_file.path())
            .arg(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            SandboxError::ExecutionFailed(format!("failed to spawn sandbox-exec: {e}"))
        })?;

        if let Some(stdin_data) = stdin {
            if let Some(mut child_stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                child_stdin
                    .write_all(stdin_data)
                    .await
                    .map_err(|e| SandboxError::Io(format!("stdin write failed: {e}")))?;
            }
        }

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                let stdout_truncated = output.stdout.len() > max_output_bytes;
                let stderr_truncated = output.stderr.len() > max_output_bytes;
                let stdout = if stdout_truncated {
                    output.stdout[..max_output_bytes].to_vec()
                } else {
                    output.stdout
                };
                let stderr = if stderr_truncated {
                    output.stderr[..max_output_bytes].to_vec()
                } else {
                    output.stderr
                };

                Ok(SandboxOutput {
                    stdout,
                    stderr,
                    exit_code: output.status.code(),
                    signal: None,
                    truncated: stdout_truncated || stderr_truncated,
                    duration_ms: elapsed_ms,
                })
            }
            Ok(Err(e)) => Err(SandboxError::ExecutionFailed(format!(
                "sandbox-exec execution error: {e}"
            ))),
            Err(_) => Err(SandboxError::Timeout { elapsed_ms }),
        }
    }
}

impl Default for SeatbeltDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn seatbelt_driver_platform() {
        let driver = SeatbeltDriver::new();
        assert_eq!(driver.platform(), "macos/seatbelt");
    }

    #[test]
    fn generate_profile_workspace_only() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(subpath \"/tmp/test-workspace\")"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(deny process-fork)"));
    }

    #[test]
    fn generate_profile_with_read_paths() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![
                PathBuf::from("/etc"),
                PathBuf::from("/usr/share"),
            ]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(subpath \"/tmp/ws\")"));
        assert!(profile.contains("(subpath \"/etc\")"));
        assert!(profile.contains("(subpath \"/usr/share\")"));
        // Read paths should only have file-read*
        assert!(profile.contains("(allow file-read* (subpath \"/etc\"))"));
    }

    #[test]
    fn generate_profile_with_write_paths() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![PathBuf::from("/tmp/output")]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow file-read* file-write* (subpath \"/tmp/output\"))"));
    }

    #[test]
    fn generate_profile_with_network() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into(), "api.example.com".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow network-outbound (remote ip \"example.com\"))"));
        assert!(profile.contains("(allow network-outbound (remote ip \"api.example.com\"))"));
    }

    #[test]
    fn generate_profile_allow_all_network() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn generate_profile_allow_fork() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            process: ProcessPolicy {
                allow_fork: true,
                timeout_secs: 60,
                max_memory_mb: None,
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // When fork is allowed, we should NOT see (deny process-fork)
        assert!(!profile.contains("(deny process-fork)"));
    }

    #[test]
    fn generate_profile_full_read_with_exclusions() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::FullRead {
                exclude: vec![PathBuf::from("/etc/passwd")],
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(deny file-read* (subpath \"/etc/passwd\"))"));
    }

    #[test]
    fn profile_for_from_capabilities() {
        let driver = SeatbeltDriver::new();
        let caps = SandboxCapabilities {
            fs_read: vec!["/tmp".into()],
            network: crate::sandbox::capabilities::NetworkPolicy::AllowAll,
            spawn_subprocess: true,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.profile_for(&caps, cwd).unwrap();

        assert!(profile.contents.contains("(allow network*)"));
        assert!(!profile.contents.contains("(deny process-fork)"));
    }
}
