//! macOS sandbox implementation using sandbox-exec
//!
//! Uses Apple's Seatbelt sandbox profile language to enforce security policies.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::error::Result;
use crate::exec::sandbox::adapter::SandboxAdapter;
use crate::exec::sandbox::capabilities::Capabilities;

/// macOS sandbox adapter using sandbox-exec
pub struct MacOSSandbox {
    workspace: PathBuf,
    profile_path: Option<PathBuf>,
}

impl MacOSSandbox {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            profile_path: None,
        }
    }

    /// Check if sandbox-exec is available on the system
    fn check_sandbox_exec() -> bool {
        std::process::Command::new("which")
            .arg("sandbox-exec")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Generate Seatbelt profile from capabilities
    fn generate_seatbelt_profile(&self, caps: &Capabilities) -> String {
        let mut profile = String::from("(version 1)\n");
        profile.push_str("(deny default)\n\n");

        // File system permissions
        if let Some(fs_caps) = &caps.filesystem {
            for path in &fs_caps.read_paths {
                profile.push_str(&format!(
                    "(allow file-read* (subpath \"{}\"))\n",
                    path.display()
                ));
            }
            for path in &fs_caps.write_paths {
                profile.push_str(&format!(
                    "(allow file-write* (subpath \"{}\"))\n",
                    path.display()
                ));
            }
        }

        // Network permissions
        if let Some(net_caps) = &caps.network {
            if net_caps.allow_outbound {
                profile.push_str("(allow network-outbound)\n");
            }
            if net_caps.allow_inbound {
                profile.push_str("(allow network-inbound)\n");
            }
        }

        // Process permissions
        if let Some(proc_caps) = &caps.process {
            if proc_caps.allow_exec {
                profile.push_str("(allow process-exec)\n");
            }
        }

        // Always allow basic system operations
        profile.push_str("\n; Basic system operations\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow mach-lookup)\n");

        profile
    }
}

#[async_trait::async_trait]
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

    fn generate_profile(
        &self,
        caps: &Capabilities,
    ) -> Result<crate::exec::sandbox::adapter::SandboxProfile> {
        use crate::exec::sandbox::adapter::SandboxProfile;
        use std::io::Write;

        // Create profile file
        let profile_path = self.workspace.join("sandbox.sb");
        let profile_content = self.generate_seatbelt_profile(caps);

        let mut file = std::fs::File::create(&profile_path)?;
        file.write_all(profile_content.as_bytes())?;

        // Determine temp workspace
        let temp_workspace = if caps.filesystem.as_ref().map_or(false, |fs| fs.temp_workspace) {
            Some(self.workspace.clone())
        } else {
            None
        };

        Ok(SandboxProfile {
            path: profile_path,
            capabilities: caps.clone(),
            platform: "macos".to_string(),
            temp_workspace,
        })
    }

    async fn execute_sandboxed(
        &self,
        command: &crate::exec::sandbox::adapter::SandboxCommand,
        profile: &crate::exec::sandbox::adapter::SandboxProfile,
    ) -> Result<crate::exec::sandbox::adapter::ExecutionResult> {
        use crate::exec::sandbox::adapter::ExecutionResult;
        use std::time::Instant;

        let start = Instant::now();

        // Build sandbox-exec command
        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-f")
            .arg(&profile.path)
            .arg(&command.program)
            .args(&command.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(working_dir) = &command.working_dir {
            cmd.current_dir(working_dir);
        }

        // Execute with timeout
        let timeout = Duration::from_secs(300); // 5 minutes default
        let output = tokio::time::timeout(timeout, cmd.output()).await??;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            sandboxed: true,
            duration_ms,
        })
    }

    fn cleanup(
        &self,
        profile: &crate::exec::sandbox::adapter::SandboxProfile,
    ) -> Result<()> {
        // Remove profile file
        if profile.path.exists() {
            std::fs::remove_file(&profile.path)?;
        }

        // Remove temp workspace if it was created
        if let Some(temp_workspace) = &profile.temp_workspace {
            if temp_workspace.exists() && temp_workspace != &self.workspace {
                std::fs::remove_dir_all(temp_workspace)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_macos_sandbox_detection() {
        let temp_dir = TempDir::new().unwrap();
        let sandbox = MacOSSandbox::new(temp_dir.path().to_path_buf());

        // Should detect sandbox-exec availability
        let supported = sandbox.is_supported();

        #[cfg(target_os = "macos")]
        {
            // On macOS, should be supported if sandbox-exec exists
            // We don't assert true because it might not be available in all environments
            println!("macOS sandbox supported: {}", supported);
        }

        #[cfg(not(target_os = "macos"))]
        {
            // On non-macOS, should not be supported
            assert!(!supported, "sandbox-exec should not be available on non-macOS");
        }
    }

    #[test]
    fn test_platform_name() {
        let temp_dir = TempDir::new().unwrap();
        let sandbox = MacOSSandbox::new(temp_dir.path().to_path_buf());

        assert_eq!(sandbox.platform_name(), "macos");
    }

    #[tokio::test]
    async fn test_macos_sandbox_execution() {
        use crate::exec::sandbox::adapter::SandboxCommand;
        use crate::exec::sandbox::capabilities::{Capabilities, FileSystemCapability};

        #[cfg(not(target_os = "macos"))]
        {
            // Skip on non-macOS
            return;
        }

        #[cfg(target_os = "macos")]
        {
            let temp_dir = TempDir::new().unwrap();
            let sandbox = MacOSSandbox::new(temp_dir.path().to_path_buf());

            if !sandbox.is_supported() {
                println!("Skipping test: sandbox-exec not available");
                return;
            }

            // Create capabilities with read access to /tmp
            let caps = Capabilities {
                filesystem: Some(FileSystemCapability {
                    read_paths: vec![PathBuf::from("/tmp")],
                    write_paths: vec![],
                    temp_workspace: false,
                }),
                network: None,
                process: None,
                environment: None,
            };

            // Generate profile
            let profile = sandbox.generate_profile(&caps).unwrap();

            // Execute simple command
            let command = SandboxCommand {
                program: "echo".to_string(),
                args: vec!["Hello, Sandbox!".to_string()],
                working_dir: None,
            };

            let result = sandbox.execute_sandboxed(&command, &profile).await.unwrap();

            assert_eq!(result.exit_code, Some(0));
            assert!(result.stdout.contains("Hello, Sandbox!"));
            assert!(result.sandboxed);

            // Cleanup
            sandbox.cleanup(&profile).unwrap();
        }
    }
}
