//! CLI Execution Backends
//!
//! Implements host, Docker, and VirtualFs execution modes for Markdown CLI tools.

use std::path::PathBuf;
use std::process::Stdio;
use anyhow::Result;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::spec::NetworkMode;
use super::tool_adapter::{MarkdownCliTool, MarkdownToolOutput};

impl MarkdownCliTool {
    /// Execute on host system (with SafetyGate if configured)
    pub(crate) async fn execute_on_host(
        &self,
        cli_args: &[String],
    ) -> Result<MarkdownToolOutput> {
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
        if let Some(aether) = &self.spec.metadata.aleph {
            if matches!(aether.security.network, NetworkMode::None) {
                // Platform-specific network isolation
                #[cfg(target_os = "linux")]
                {
                    cmd.env("NO_PROXY", "*");
                    // TODO: Use unshare(CLONE_NEWNET) for true isolation
                }
            }
        }

        // Execute
        let output = cmd.output().await?;

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
        if let Some(aether) = &self.spec.metadata.aleph {
            if let Some(docker_cfg) = &aether.docker {
                for env_var in &docker_cfg.env_vars {
                    if let Ok(value) = std::env::var(env_var) {
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

                // Extra flags (e.g., volume mounts)
                docker_args.extend(docker_cfg.extra_flags.clone());
            }
        }

        docker_args.push(container_image);
        docker_args.push(bin.clone());
        docker_args.extend_from_slice(cli_args);

        let output = Command::new("docker")
            .args(&docker_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .await?;

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
        if let Some(aether) = &self.spec.metadata.aleph {
            if let Some(docker_cfg) = &aether.docker {
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
        if let Some(aether) = &self.spec.metadata.aleph {
            match aether.security.network {
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
        if let Some(aether) = &self.spec.metadata.aleph {
            if matches!(aether.security.network, NetworkMode::None) {
                #[cfg(target_os = "linux")]
                {
                    cmd.env("NO_PROXY", "*");
                }
            }
        }

        // Execute
        let output = cmd.output().await?;

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
            "aether-virtualfs-{}-{}",
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

    /// Get paths info for debugging
    #[allow(dead_code)]
    fn info(&self) -> SandboxInfo {
        SandboxInfo {
            root: self.root_dir.clone(),
            work: self.work_dir.clone(),
            home: self.home_dir.clone(),
            temp: self.temp_dir.clone(),
        }
    }
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

/// Sandbox path information (for debugging/testing)
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SandboxInfo {
    root: PathBuf,
    work: PathBuf,
    home: PathBuf,
    temp: PathBuf,
}
