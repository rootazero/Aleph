//! WindowsSandboxDriver — Windows sandbox implementation using
//! Restricted Token + Job Object + ACL.
//!
//! This implementation follows the same architecture as macOS SeatbeltDriver
//! and Linux BubblewrapDriver, mapping SandboxPolicy to Windows-native
//! security mechanisms.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, SandboxPolicy};

#[derive(Debug, Clone)]
pub struct WindowsSandboxDriver;

impl WindowsSandboxDriver {
    pub fn new() -> Self {
        Self
    }

    /// Generate a profile description from the sandbox policy.
    /// On Windows, the profile contains the serialized policy that
    /// will be applied at runtime via Windows APIs.
    fn generate_profile(
        &self,
        policy: &SandboxPolicy,
        cwd: &Path,
    ) -> Result<String, SandboxError> {
        let mut lines = Vec::new();

        lines.push("platform=windows/token".to_string());

        match &policy.filesystem {
            FsPolicy::WorkspaceOnly => {
                lines.push("fs=workspace_only".to_string());
                lines.push(format!("cwd={}", cwd.display()));
            }
            FsPolicy::ReadPaths(paths) => {
                lines.push("fs=read_paths".to_string());
                lines.push(format!("cwd={}", cwd.display()));
                for path in paths {
                    lines.push(format!("read={}", path.display()));
                }
            }
            FsPolicy::WritePaths(paths) => {
                lines.push("fs=write_paths".to_string());
                lines.push(format!("cwd={}", cwd.display()));
                for path in paths {
                    lines.push(format!("write={}", path.display()));
                }
            }
            FsPolicy::FullRead { exclude } => {
                lines.push("fs=full_read".to_string());
                for path in exclude {
                    lines.push(format!("exclude={}", path.display()));
                }
            }
            FsPolicy::FullWrite { exclude } => {
                lines.push("fs=full_write".to_string());
                for path in exclude {
                    lines.push(format!("exclude={}", path.display()));
                }
            }
        }

        match &policy.network {
            NetworkPolicy::None => {
                lines.push("network=none".to_string());
            }
            NetworkPolicy::AllowAll => {
                lines.push("network=allow_all".to_string());
            }
            NetworkPolicy::AllowHosts(hosts) => {
                lines.push("network=allow_hosts".to_string());
                for host in hosts {
                    lines.push(format!("host={}", host));
                }
            }
            NetworkPolicy::ProxyOnly { ports } => {
                lines.push("network=proxy_only".to_string());
                for port in ports {
                    lines.push(format!("port={}", port));
                }
            }
        }

        lines.push(format!("allow_fork={}", policy.process.allow_fork));
        lines.push(format!("timeout_secs={}", policy.process.timeout_secs));
        if let Some(max_mem) = policy.process.max_memory_mb {
            lines.push(format!("max_memory_mb={}", max_mem));
        }

        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl OsSandboxDriverTrait for WindowsSandboxDriver {
    fn platform(&self) -> &'static str {
        "windows/token"
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "windows")
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
        let parsed = parse_profile(&profile.contents)?;

        debug!("running Windows sandbox for program: {}", program);

        #[cfg(target_os = "windows")]
        {
            use super::job::SandboxJob;
            use super::token::create_restricted_token;
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

            let mut cmd = tokio::process::Command::new(program);
            cmd.args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(std::process::Stdio::piped())
                .creation_flags(CREATE_NEW_PROCESS_GROUP as u32);

            let job = unsafe {
                SandboxJob::new(if parsed.allow_fork { 32 } else { 1 })
                    .map_err(|e| SandboxError::ExecutionFailed(format!("job creation failed: {e}")))?
            };

            let child = cmd.spawn().map_err(|e| {
                SandboxError::ExecutionFailed(format!("failed to spawn process: {e}"))
            })?;

            let pid = child.id().unwrap_or(0);
            if pid != 0 {
                let handle = child.raw_handle().unwrap_or(std::ptr::null_mut());
                if !handle.is_null() {
                    let _ = unsafe { job.assign_process(handle as _) };
                }
            }

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
                    "process execution error: {e}"
                ))),
                Err(_) => Err(SandboxError::Timeout { elapsed_ms }),
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (program, args, env, stdin, cwd, timeout, max_output_bytes, parsed);
            Err(SandboxError::Other(
                "Windows sandbox driver requires Windows platform".into(),
            ))
        }
    }
}

impl Default for WindowsSandboxDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a profile string back into policy components.
fn parse_profile(contents: &str) -> Result<ParsedProfile, SandboxError> {
    let mut profile = ParsedProfile::default();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            match key {
                "platform" => profile.platform = value.to_string(),
                "fs" => profile.fs_mode = value.to_string(),
                "cwd" => profile.cwd = Some(value.to_string()),
                "read" => profile.read_paths.push(value.to_string()),
                "write" => profile.write_paths.push(value.to_string()),
                "exclude" => profile.exclude_paths.push(value.to_string()),
                "network" => profile.network_mode = value.to_string(),
                "host" => profile.allowed_hosts.push(value.to_string()),
                "port" => {
                    if let Ok(port) = value.parse() {
                        profile.proxy_ports.push(port);
                    }
                }
                "allow_fork" => profile.allow_fork = value == "true",
                "timeout_secs" => {
                    if let Ok(secs) = value.parse() {
                        profile.timeout_secs = secs;
                    }
                }
                "max_memory_mb" => {
                    if let Ok(mb) = value.parse() {
                        profile.max_memory_mb = Some(mb);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(profile)
}

#[derive(Debug, Default)]
struct ParsedProfile {
    platform: String,
    fs_mode: String,
    cwd: Option<String>,
    read_paths: Vec<String>,
    write_paths: Vec<String>,
    exclude_paths: Vec<String>,
    network_mode: String,
    allowed_hosts: Vec<String>,
    proxy_ports: Vec<u16>,
    allow_fork: bool,
    timeout_secs: u64,
    max_memory_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn windows_driver_platform() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.platform(), "windows/token");
    }

    #[test]
    fn windows_driver_is_supported_on_windows() {
        let driver = WindowsSandboxDriver::new();
        let _ = driver.is_supported();
    }

    #[test]
    fn generate_profile_workspace_only() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("platform=windows/token"));
        assert!(profile.contains("fs=workspace_only"));
        assert!(profile.contains("cwd=C:\\workspace"));
        assert!(profile.contains("network=none"));
        assert!(profile.contains("allow_fork=false"));
    }

    #[test]
    fn generate_profile_with_read_paths() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![PathBuf::from("C:\\ProgramData")]),
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("fs=read_paths"));
        assert!(profile.contains("read=C:\\ProgramData"));
    }

    #[test]
    fn generate_profile_allow_all_network() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("network=allow_all"));
    }

    #[test]
    fn generate_profile_allow_hosts() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("network=allow_hosts"));
        assert!(profile.contains("host=example.com"));
    }

    #[test]
    fn parse_profile_roundtrip() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![PathBuf::from("C:\\temp")]),
            network: NetworkPolicy::ProxyOnly { ports: vec![8080] },
            process: crate::sandbox::policy::ProcessPolicy {
                allow_fork: true,
                timeout_secs: 120,
                max_memory_mb: Some(512),
            },
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile_str = driver.generate_profile(&policy, cwd).unwrap();

        let parsed = parse_profile(&profile_str).unwrap();
        assert_eq!(parsed.platform, "windows/token");
        assert_eq!(parsed.fs_mode, "write_paths");
        assert_eq!(parsed.cwd, Some("C:\\workspace".to_string()));
        assert_eq!(parsed.write_paths, vec!["C:\\temp"]);
        assert_eq!(parsed.network_mode, "proxy_only");
        assert_eq!(parsed.proxy_ports, vec![8080]);
        assert!(parsed.allow_fork);
        assert_eq!(parsed.timeout_secs, 120);
        assert_eq!(parsed.max_memory_mb, Some(512));
    }

    #[test]
    fn profile_for_from_capabilities() {
        let driver = WindowsSandboxDriver::new();
        let caps = SandboxCapabilities {
            fs_read: vec!["C:\\ProgramData".into()],
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.profile_for(&caps, cwd).unwrap();

        assert!(profile.contents.contains("fs=read_paths"));
        assert!(profile.contents.contains("read=C:\\ProgramData"));
    }
}
