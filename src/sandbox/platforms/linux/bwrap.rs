use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::common::{is_wsl, wsl_version, LINUX_PLATFORM_DEFAULT_READ_ROOTS};
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

const BWRAP_CANDIDATES: [&str; 2] = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxSandboxOptions {
    pub mount_proc: bool,
    pub no_new_privs: bool,
    pub include_platform_defaults: bool,
}

impl Default for LinuxSandboxOptions {
    fn default() -> Self {
        Self {
            mount_proc: true,
            no_new_privs: true,
            include_platform_defaults: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BubblewrapDriver {
    options: LinuxSandboxOptions,
}

impl BubblewrapDriver {
    pub fn new() -> Self {
        Self {
            options: LinuxSandboxOptions::default(),
        }
    }

    pub fn with_options(options: LinuxSandboxOptions) -> Self {
        Self { options }
    }

    fn find_bwrap(&self) -> Option<PathBuf> {
        for path in &BWRAP_CANDIDATES {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }

        if let Ok(paths) = std::env::var("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join("bwrap");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        None
    }

    fn check_wsl(&self) {
        if is_wsl() {
            match wsl_version() {
                Some(1) => {
                    warn!(
                        "WSL1 detected. Bubblewrap sandbox may not work correctly \
                         due to lack of proper Linux namespace support. \
                         Consider upgrading to WSL2."
                    );
                }
                Some(2) => {
                    debug!("WSL2 detected. Bubblewrap sandbox should work correctly.");
                }
                _ => {
                    warn!(
                        "WSL detected but version unknown. Sandbox behavior may be unpredictable."
                    );
                }
            }
        }
    }

    fn generate_args(
        &self,
        policy: &SandboxPolicy,
        cwd: &Path,
    ) -> Result<Vec<String>, SandboxError> {
        self.check_wsl();

        let mut args = Vec::new();

        args.push("--new-session".into());
        args.push("--die-with-parent".into());
        args.push("--unshare-user".into());

        if !policy.process.allow_fork {
            args.push("--unshare-pid".into());
            args.push("--cap-drop".into());
            args.push("ALL".into());
        }

        match &policy.network {
            NetworkPolicy::None => {
                args.push("--unshare-net".into());
            }
            NetworkPolicy::AllowAll => {}
            NetworkPolicy::AllowHosts(hosts) => {
                warn!(
                    "AllowHosts network policy requested but not fully supported on Linux. \
                     Falling back to --unshare-net. Allowed hosts: {:?}",
                    hosts
                );
                args.push("--unshare-net".into());
            }
            NetworkPolicy::ProxyOnly { ports } => {
                warn!(
                    "ProxyOnly network policy requested but not fully supported on Linux. \
                     Falling back to --unshare-net. Proxy ports: {:?}",
                    ports
                );
                args.push("--unshare-net".into());
            }
        }

        self.add_fs_args(&mut args, &policy.filesystem, cwd)?;

        if self.options.mount_proc {
            args.push("--proc".into());
            args.push("/proc".into());
        }
        args.push("--dev".into());
        args.push("/dev".into());

        let cwd_str = cwd.to_str().ok_or_else(|| {
            SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
        })?;
        args.push("--chdir".into());
        args.push(cwd_str.into());

        args.push("--".into());

        Ok(args)
    }

    fn add_fs_args(
        &self,
        args: &mut Vec<String>,
        fs: &FsPolicy,
        cwd: &Path,
    ) -> Result<(), SandboxError> {
        match fs {
            FsPolicy::WorkspaceOnly => {
                args.push("--tmpfs".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());
                args.push("--dir".into());
                args.push("/tmp".into());
                args.push("--tmpfs".into());
                args.push("/tmp".into());

                if self.options.include_platform_defaults {
                    for root in LINUX_PLATFORM_DEFAULT_READ_ROOTS {
                        if Path::new(root).exists() {
                            args.push("--ro-bind".into());
                            args.push(root.to_string());
                            args.push(root.to_string());
                        }
                    }
                }

                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());
            }
            FsPolicy::ReadPaths(paths) => {
                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());

                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--ro-bind".into());
                    args.push(path_str.into());
                    args.push(path_str.into());
                }
            }
            FsPolicy::WritePaths(paths) => {
                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());

                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--bind".into());
                    args.push(path_str.into());
                    args.push(path_str.into());
                }
            }
            FsPolicy::FullRead { exclude } => {
                args.push("--ro-bind".into());
                args.push("/".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());

                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--tmpfs".into());
                    args.push(path_str.into());
                }
            }
            FsPolicy::FullWrite { exclude } => {
                args.push("--bind".into());
                args.push("/".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());

                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--tmpfs".into());
                    args.push(path_str.into());
                }
            }
        }
        Ok(())
    }

    fn apply_no_new_privs(&self, cmd: &mut Command) {
        if self.options.no_new_privs {
            debug!("Applying PR_SET_NO_NEW_PRIVS via pre-exec");
            unsafe {
                cmd.pre_exec(|| {
                    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS is a well-defined
                    // Linux syscall that cannot fail with valid arguments.
                    let rc = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
                    if rc != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }
}

#[async_trait]
impl OsSandboxDriverTrait for BubblewrapDriver {
    fn platform(&self) -> &'static str {
        "linux/bwrap"
    }

    fn is_supported(&self) -> bool {
        self.find_bwrap().is_some()
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        let policy = SandboxPolicy::from(capabilities);
        let args = self.generate_args(&policy, cwd)?;
        let contents = args.join("\n");
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
        let bwrap_path = self
            .find_bwrap()
            .ok_or_else(|| SandboxError::ExecutionFailed("bubblewrap (bwrap) not found".into()))?;

        let bwrap_args: Vec<String> = profile.contents.lines().map(|s| s.to_string()).collect();

        debug!("running bubblewrap with {} arguments", bwrap_args.len());

        let mut cmd = Command::new(bwrap_path);
        cmd.args(&bwrap_args)
            .arg(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped());

        self.apply_no_new_privs(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| SandboxError::ExecutionFailed(format!("failed to spawn bwrap: {e}")))?;

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
                "bwrap execution error: {e}"
            ))),
            Err(_) => Err(SandboxError::Timeout { elapsed_ms }),
        }
    }
}

impl Default for BubblewrapDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl BubblewrapDriver {
    pub fn options(&self) -> LinuxSandboxOptions {
        self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bubblewrap_driver_platform() {
        let driver = BubblewrapDriver::new();
        assert_eq!(driver.platform(), "linux/bwrap");
    }

    #[test]
    fn generate_args_workspace_only() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--new-session".into()));
        assert!(args.contains(&"--die-with-parent".into()));
        assert!(args.contains(&"--unshare-user".into()));
        assert!(args.contains(&"--unshare-net".into()));
        assert!(args.contains(&"--unshare-pid".into()));
        assert!(args.contains(&"--cap-drop".into()));
        assert!(args.contains(&"ALL".into()));
        assert!(args.contains(&"--tmpfs".into()));
        assert!(args.contains(&"/".into()));
        assert!(args.contains(&"--bind".into()));
        assert!(args.contains(&"/tmp/test-workspace".into()));
    }

    #[test]
    fn generate_args_workspace_only_without_platform_defaults() {
        let driver = BubblewrapDriver::with_options(LinuxSandboxOptions {
            mount_proc: true,
            no_new_privs: true,
            include_platform_defaults: false,
        });
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--tmpfs".into()));
        assert!(!args.iter().any(|a| a == "/usr"));
    }

    #[test]
    fn generate_args_with_read_paths() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![PathBuf::from("/etc")]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--ro-bind".into()));
        assert!(args.contains(&"/etc".into()));
    }

    #[test]
    fn generate_args_allow_all_network() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(!args.contains(&"--unshare-net".into()));
    }

    #[test]
    fn generate_args_allow_hosts_fallback() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--unshare-net".into()));
    }

    #[test]
    fn generate_args_allow_fork() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            process: ProcessPolicy {
                allow_fork: true,
                timeout_secs: 60,
                max_memory_mb: None,
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(!args.contains(&"--unshare-pid".into()));
        assert!(!args.contains(&"--cap-drop".into()));
    }
}
