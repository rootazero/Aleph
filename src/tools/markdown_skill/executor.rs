//! CLI Execution Backends
//!
//! Implements host, Docker, and VirtualFs execution modes for Markdown CLI tools.

use anyhow::Result;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Default execution timeout for CLI skills (5 minutes).
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);

use super::spec::NetworkMode;
use super::tool_adapter::{MarkdownCliTool, MarkdownToolOutput};

impl MarkdownCliTool {
    /// Resolve the execution timeout: skill-specific override or global default.
    fn execution_timeout(&self) -> Duration {
        self.spec
            .metadata
            .aleph
            .as_ref()
            .and_then(|a| a.timeout_secs)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_EXECUTION_TIMEOUT)
    }

    /// Execute on host system (with SafetyGate if configured)
    pub(crate) async fn execute_on_host(&self, cli_args: &[String]) -> Result<MarkdownToolOutput> {
        // Get primary binary name
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified in skill metadata"))?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            args = ?cli_args,
            "Executing CLI tool on host"
        );

        // Build command
        let mut cmd = Command::new(bin);
        cmd.args(cli_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Apply network restrictions if specified
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if matches!(aleph_meta.security.network, NetworkMode::None) {
                // Platform-specific network isolation
                #[cfg(target_os = "linux")]
                {
                    cmd.env("NO_PROXY", "*");
                    // TODO: Use unshare(CLONE_NEWNET) for true isolation
                }
            }
        }

        // Execute with timeout
        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Skill '{}' timed out after {}s",
                    self.spec.name,
                    timeout.as_secs()
                )
            })??;

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Execute in Docker container with proper env and error handling
    pub(crate) async fn execute_in_docker(
        &self,
        cli_args: &[String],
    ) -> Result<MarkdownToolOutput> {
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        // Get Docker image (STRICT: must be configured)
        let container_image = self.get_docker_image()?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            image = %container_image,
            args = ?cli_args,
            "Executing CLI tool in Docker sandbox"
        );

        let mut docker_args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--network".to_string(),
            self.get_docker_network_mode(),
            "--read-only".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,noexec,nosuid,size=100m".to_string(),
        ];

        // Pass environment variables
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if let Some(docker_cfg) = &aleph_meta.docker {
                for env_var in &docker_cfg.env_vars {
                    // Validate env var name: must be a valid identifier [A-Za-z_][A-Za-z0-9_]*
                    if env_var.is_empty()
                        || !env_var.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                        || !env_var
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        warn!(
                            env_var = %env_var,
                            "Skipping env var with invalid name (must match [A-Za-z_][A-Za-z0-9_]*)"
                        );
                        continue;
                    }
                    if let Ok(value) = std::env::var(env_var) {
                        // Sanitize: reject values containing newlines (could break Docker CLI parsing)
                        if value.contains('\n') || value.contains('\r') {
                            warn!(
                                env_var = %env_var,
                                "Skipping env var with newline characters (security risk)"
                            );
                            continue;
                        }
                        docker_args.push("-e".to_string());
                        docker_args.push(format!("{}={}", env_var, value));
                        tracing::debug!(env_var = %env_var, "Passing env var to container");
                    } else {
                        warn!(
                            env_var = %env_var,
                            "Required env var not found in host environment"
                        );
                    }
                }

                // Extra flags — filtered through allowlist to prevent sandbox escape
                for flag in &docker_cfg.extra_flags {
                    if is_allowed_docker_flag(flag) {
                        docker_args.push(flag.clone());
                    } else {
                        warn!(
                            flag = %flag,
                            tool = %self.spec.name,
                            "Blocked disallowed Docker flag from skill config"
                        );
                    }
                }
            }
        }

        docker_args.push(container_image);
        docker_args.push(bin.clone());
        docker_args.extend_from_slice(cli_args);

        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(
            timeout,
            Command::new("docker")
                .args(&docker_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Docker skill '{}' timed out after {}s",
                self.spec.name,
                timeout.as_secs()
            )
        })??;

        // Enhanced exit code handling
        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr);

            match exit_code {
                125 => anyhow::bail!(
                    "Docker runtime error (container failed to start): {}",
                    stderr
                ),
                126 => anyhow::bail!("Command cannot be executed in container: {}", stderr),
                127 => anyhow::bail!(
                    "Command '{}' not found in container image '{}'. \
                    Check metadata.aleph.docker.image configuration.",
                    bin,
                    self.get_docker_image().unwrap_or_default()
                ),
                137 => anyhow::bail!("Container killed (OOM or SIGKILL): {}", stderr),
                _ => {
                    // Tool itself failed (non-zero exit), return output
                    warn!(
                        tool = %self.spec.name,
                        exit_code = exit_code,
                        "Tool execution failed"
                    );
                }
            }
        }

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Get Docker image (STRICT: must be configured or known)
    fn get_docker_image(&self) -> Result<String> {
        // Priority 1: Explicit configuration
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if let Some(docker_cfg) = &aleph_meta.docker {
                return Ok(docker_cfg.image.clone());
            }
        }

        // Priority 2: Hardcoded mapping for common tools
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        let known_image = match bin.as_str() {
            "gh" => Some("ghcr.io/cli/cli:latest"),
            "kubectl" => Some("bitnami/kubectl:latest"),
            "aws" => Some("amazon/aws-cli:latest"),
            "gcloud" => Some("google/cloud-sdk:alpine"),
            "terraform" => Some("hashicorp/terraform:latest"),
            "helm" => Some("alpine/helm:latest"),
            "ffmpeg" => Some("linuxserver/ffmpeg:latest"),
            "yt-dlp" => Some("jauderho/yt-dlp:latest"),
            _ => None,
        };

        if let Some(image) = known_image {
            info!(
                bin = %bin,
                image = %image,
                "Using known Docker image mapping"
            );
            return Ok(image.to_string());
        }

        // Priority 3: FAIL (no blind fallback to alpine)
        anyhow::bail!(
            "Docker execution for '{}' requires 'metadata.aleph.docker.image' configuration. \
            Binary '{}' has no known Docker image mapping.",
            self.spec.name,
            bin
        )
    }

    fn get_docker_network_mode(&self) -> String {
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            match aleph_meta.security.network {
                NetworkMode::None => "none".to_string(),
                NetworkMode::Local => "bridge".to_string(),
                NetworkMode::Internet => "bridge".to_string(),
            }
        } else {
            "bridge".to_string()
        }
    }

    /// Execute in VirtualFs sandbox (lightweight isolation)
    ///
    /// Provides filesystem isolation through:
    /// - Temporary isolated working directory
    /// - Environment variable redirection (HOME, TMPDIR, PWD)
    /// - Read-only access to real filesystem
    /// - Writable temporary filesystem
    /// - Automatic cleanup after execution
    pub(crate) async fn execute_in_virtualfs(
        &self,
        cli_args: &[String],
    ) -> Result<MarkdownToolOutput> {
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        // Create isolated sandbox environment
        let sandbox = VirtualFsSandbox::new(&self.spec.name)?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            sandbox_dir = %sandbox.root_dir.display(),
            args = ?cli_args,
            "Executing CLI tool in VirtualFs sandbox"
        );

        // Build command with isolated environment
        let mut cmd = Command::new(bin);
        cmd.args(cli_args)
            .current_dir(&sandbox.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Apply sandbox environment variables
        sandbox.apply_env(&mut cmd);

        // Apply network restrictions if specified
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if matches!(aleph_meta.security.network, NetworkMode::None) {
                #[cfg(target_os = "linux")]
                {
                    cmd.env("NO_PROXY", "*");
                }
            }
        }

        // Execute with timeout
        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "VirtualFs skill '{}' timed out after {}s",
                    self.spec.name,
                    timeout.as_secs()
                )
            })??;

        // Cleanup happens when sandbox is dropped

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

/// VirtualFs Sandbox Environment
///
/// Provides lightweight filesystem isolation by creating a temporary
/// directory structure and redirecting environment variables.
///
/// ## Isolation Strategy
///
/// - **Working Directory**: Isolated temp directory for execution
/// - **Home Directory**: Sandbox-specific home for config files
/// - **Temp Directory**: Sandbox-specific temp for temporary files
/// - **Real Filesystem**: Read-only access (via normal file paths)
///
/// ## Security
///
/// - All writes go to sandbox temp directories
/// - Real filesystem remains unmodified (unless tool uses absolute paths)
/// - Automatic cleanup on drop
///
/// ## Limitations
///
/// - Not true filesystem isolation (tools can still access real FS via absolute paths)
/// - Best for well-behaved CLI tools that respect environment variables
/// - For untrusted code, use Docker sandbox instead
struct VirtualFsSandbox {
    /// Root directory of the sandbox (will be cleaned up)
    root_dir: PathBuf,

    /// Working directory for command execution
    work_dir: PathBuf,

    /// Isolated home directory
    home_dir: PathBuf,

    /// Isolated temp directory
    temp_dir: PathBuf,
}

impl VirtualFsSandbox {
    /// Create a new VirtualFs sandbox
    fn new(tool_name: &str) -> Result<Self> {
        // Create root sandbox directory with unique name
        let root_dir = std::env::temp_dir().join(format!(
            "aleph-virtualfs-{}-{}",
            tool_name,
            uuid::Uuid::new_v4()
        ));

        std::fs::create_dir_all(&root_dir)?;

        // Create subdirectories
        let work_dir = root_dir.join("work");
        let home_dir = root_dir.join("home");
        let temp_dir = root_dir.join("tmp");

        std::fs::create_dir_all(&work_dir)?;
        std::fs::create_dir_all(&home_dir)?;
        std::fs::create_dir_all(&temp_dir)?;

        debug!(
            root = %root_dir.display(),
            "Created VirtualFs sandbox"
        );

        Ok(Self {
            root_dir,
            work_dir,
            home_dir,
            temp_dir,
        })
    }

    /// Apply sandbox environment variables to command
    fn apply_env(&self, cmd: &mut Command) {
        // Redirect HOME to sandbox home
        cmd.env("HOME", &self.home_dir);

        // Redirect TMPDIR/TEMP/TMP to sandbox temp
        cmd.env("TMPDIR", &self.temp_dir);
        cmd.env("TEMP", &self.temp_dir);
        cmd.env("TMP", &self.temp_dir);

        // Set PWD to sandbox work directory
        cmd.env("PWD", &self.work_dir);

        // Clear potentially dangerous environment variables
        cmd.env_remove("LD_PRELOAD");
        cmd.env_remove("DYLD_INSERT_LIBRARIES");
        cmd.env_remove("DYLD_LIBRARY_PATH");
        cmd.env_remove("LD_LIBRARY_PATH");

        debug!(
            home = %self.home_dir.display(),
            tmp = %self.temp_dir.display(),
            pwd = %self.work_dir.display(),
            "Applied VirtualFs environment"
        );
    }
}

/// Allowlist of safe Docker flags that skills may use.
///
/// Only explicitly permitted flags pass through. Unknown flags are rejected
/// to prevent sandbox escapes via novel or future Docker flags.
fn is_allowed_docker_flag(flag: &str) -> bool {
    // Reject flags containing whitespace — prevents "-v /host:/container" bypass
    if flag.contains(char::is_whitespace) {
        return false;
    }

    // Must start with '-' to be a flag
    if !flag.starts_with('-') {
        return false;
    }

    let flag_lower = flag.to_lowercase();

    // Extract the flag name (before '=' if present)
    let flag_name = flag_lower.split('=').next().unwrap_or(&flag_lower);
    let flag_name = flag_name.trim_start_matches('-');

    // Allowlist of safe flags — only these are permitted
    const ALLOWED: &[&str] = &[
        "memory",
        "m",
        "cpus",
        "cpu-shares",
        "c",
        "cpu-period",
        "cpu-quota",
        "memory-swap",
        "memory-reservation",
        "workdir",
        "w",
        "env",
        "e",
        "env-file",
        "label",
        "l",
        "name",
        "hostname",
        "h",
        "tmpfs",
        "read-only",
        "rm",
        "interactive",
        "i",
        "tty",
        "t",
        "detach",
        "d",
        "stop-timeout",
        "shm-size",
        "log-opt",
    ];

    ALLOWED.contains(&flag_name)
}

impl Drop for VirtualFsSandbox {
    fn drop(&mut self) {
        // Clean up sandbox directory
        if let Err(e) = std::fs::remove_dir_all(&self.root_dir) {
            warn!(
                error = %e,
                sandbox_dir = %self.root_dir.display(),
                "Failed to clean up VirtualFs sandbox"
            );
        } else {
            debug!(
                sandbox_dir = %self.root_dir.display(),
                "Cleaned up VirtualFs sandbox"
            );
        }
    }
}
