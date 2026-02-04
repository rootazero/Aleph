//! CLI Execution Backends
//!
//! Implements host and Docker execution modes for Markdown CLI tools.

use std::process::Stdio;
use anyhow::Result;
use tokio::process::Command;
use tracing::{info, warn};

use super::spec::{NetworkMode, SandboxMode};
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
        if let Some(aether) = &self.spec.metadata.aether {
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
        if let Some(aether) = &self.spec.metadata.aether {
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
                    Check metadata.aether.docker.image configuration.",
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
        if let Some(aether) = &self.spec.metadata.aether {
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
            "Docker execution for '{}' requires 'metadata.aether.docker.image' configuration. \
            Binary '{}' has no known Docker image mapping.",
            self.spec.name,
            bin
        )
    }

    fn get_docker_network_mode(&self) -> String {
        if let Some(aether) = &self.spec.metadata.aether {
            match aether.security.network {
                NetworkMode::None => "none".to_string(),
                NetworkMode::Local => "bridge".to_string(),
                NetworkMode::Internet => "bridge".to_string(),
            }
        } else {
            "bridge".to_string()
        }
    }
}
