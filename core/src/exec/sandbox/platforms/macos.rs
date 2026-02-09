//! macOS sandbox implementation using sandbox-exec
//!
//! Uses Apple's Seatbelt sandbox profile language to enforce security policies.

use crate::error::{AlephError, Result};
use crate::exec::sandbox::adapter::{
    ExecutionResult, SandboxAdapter, SandboxCommand, SandboxProfile,
};
use crate::exec::sandbox::capabilities::{
    Capabilities, FileSystemCapability, NetworkCapability,
};
use crate::exec::sandbox::profile::ProfileGenerator;
use async_trait::async_trait;
use std::time::Instant;
use tokio::process::Command;

/// macOS sandbox adapter using sandbox-exec
pub struct MacOSSandbox;

impl MacOSSandbox {
    pub fn new() -> Self {
        Self
    }

    /// Check if sandbox-exec is available on the system
    fn check_sandbox_exec() -> bool {
        std::process::Command::new("which")
            .arg("sandbox-exec")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

#[async_trait]
impl SandboxAdapter for MacOSSandbox {
    fn is_supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            Self::check_sandbox_exec()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn platform_name(&self) -> &str {
        "macos"
    }

    fn generate_profile(&self, caps: &Capabilities) -> Result<SandboxProfile> {
        let mut profile = String::from("(version 1)\n");
        profile.push_str("(deny default)\n\n");

        // Allow basic system calls
        profile.push_str(";; Allow basic system operations\n");
        profile.push_str("(allow process-exec*)\n");
        profile.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
        profile.push_str("(allow file-read* (subpath \"/usr/bin\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/urandom\"))\n\n");

        // Temporary workspace tracking
        let mut temp_workspace = None;

        // Generate filesystem rules
        profile.push_str(";; Filesystem permissions\n");
        for fs_cap in &caps.filesystem {
            match fs_cap {
                FileSystemCapability::ReadOnly { path } => {
                    profile.push_str(&format!(
                        "(allow file-read* (subpath \"{}\"))\n",
                        path.display()
                    ));
                }
                FileSystemCapability::ReadWrite { path } => {
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        path.display()
                    ));
                }
                FileSystemCapability::TempWorkspace => {
                    let temp_dir = ProfileGenerator::create_temp_workspace()?;
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        temp_dir.display()
                    ));
                    temp_workspace = Some(temp_dir);
                }
            }
        }
        profile.push_str("\n");

        // Network rules
        profile.push_str(";; Network permissions\n");
        match &caps.network {
            NetworkCapability::Deny => {
                profile.push_str("(deny network*)\n");
            }
            NetworkCapability::AllowDomains(domains) => {
                for domain in domains {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote tcp \"{}:*\"))\n",
                        domain
                    ));
                }
            }
            NetworkCapability::AllowAll => {
                profile.push_str("(allow network*)\n");
            }
        }

        // Write profile to temp file
        let profile_path = ProfileGenerator::write_temp_profile(&profile, ".sb")?;

        Ok(SandboxProfile {
            path: profile_path,
            capabilities: caps.clone(),
            platform: "macos".to_string(),
            temp_workspace,
        })
    }

    async fn execute_sandboxed(
        &self,
        command: &SandboxCommand,
        profile: &SandboxProfile,
    ) -> Result<ExecutionResult> {
        let start = Instant::now();

        // Build sandbox-exec command
        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-f").arg(&profile.path);
        cmd.arg(&command.program);
        cmd.args(&command.args);

        if let Some(ref working_dir) = command.working_dir {
            cmd.current_dir(working_dir);
        }

        // Set timeout
        let timeout = std::time::Duration::from_secs(profile.capabilities.process.max_execution_time);

        // Execute with timeout
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| AlephError::ExecutionTimeout {
                timeout_secs: profile.capabilities.process.max_execution_time,
            })??;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            sandboxed: true,
            duration_ms,
        })
    }

    fn cleanup(&self, profile: &SandboxProfile) -> Result<()> {
        // Remove profile file
        if profile.path.exists() {
            std::fs::remove_file(&profile.path)?;
        }

        // Remove temp workspace
        if let Some(ref workspace) = profile.temp_workspace {
            if workspace.exists() {
                std::fs::remove_dir_all(workspace)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macos_sandbox_supported() {
        let sandbox = MacOSSandbox::new();

        #[cfg(target_os = "macos")]
        {
            // On macOS, should be supported if sandbox-exec exists
            let supported = sandbox.is_supported();
            println!("macOS sandbox supported: {}", supported);
        }

        #[cfg(not(target_os = "macos"))]
        {
            // On non-macOS, should not be supported
            assert!(!sandbox.is_supported(), "sandbox-exec should not be available on non-macOS");
        }
    }

    #[test]
    fn test_platform_name() {
        let sandbox = MacOSSandbox::new();
        assert_eq!(sandbox.platform_name(), "macos");
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_macos_sandbox_execution() {
        let sandbox = MacOSSandbox::new();
        if !sandbox.is_supported() {
            println!("Skipping test: sandbox-exec not available");
            return;
        }

        let caps = Capabilities::default();
        let profile = sandbox.generate_profile(&caps).unwrap();

        let command = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            working_dir: None,
        };

        let result = sandbox.execute_sandboxed(&command, &profile).await.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
        assert!(result.sandboxed);

        sandbox.cleanup(&profile).unwrap();
    }
}

