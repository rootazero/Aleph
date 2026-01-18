//! macOS System Information Implementation
//!
//! Provides `MacOsSystemInfo` which implements `SystemInfoProvider` for macOS.
//! Uses standard Rust libraries and system calls, no external CLI dependencies.

use async_trait::async_trait;
use std::process::Command;

use super::{SystemInfo, SystemInfoProvider};
use crate::error::{AetherError, Result};

/// macOS system information implementation
#[derive(Debug, Default, Clone)]
pub struct MacOsSystemInfo;

impl MacOsSystemInfo {
    /// Create a new MacOsSystemInfo instance
    pub fn new() -> Self {
        Self
    }

    /// Get memory info using sysctl (macOS)
    fn get_memory_info() -> (u64, u64) {
        // Try to get total memory via sysctl
        let total = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);

        // Get page size and free pages for available memory
        let page_size = Command::new("sysctl")
            .args(["-n", "hw.pagesize"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(4096);

        // vm_stat gives us free pages (this is approximate)
        let available = Command::new("vm_stat")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|output| {
                // Parse "Pages free:" line
                let free_pages: u64 = output
                    .lines()
                    .find(|l| l.contains("Pages free:"))
                    .and_then(|l| {
                        l.split(':')
                            .nth(1)
                            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
                    })
                    .unwrap_or(0);

                // Parse "Pages inactive:" line (can be reclaimed)
                let inactive_pages: u64 = output
                    .lines()
                    .find(|l| l.contains("Pages inactive:"))
                    .and_then(|l| {
                        l.split(':')
                            .nth(1)
                            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
                    })
                    .unwrap_or(0);

                (free_pages + inactive_pages) * page_size
            })
            .unwrap_or(0);

        (total, available)
    }

    /// Get OS version using sw_vers
    fn get_os_version() -> String {
        Command::new("sw_vers")
            .args(["-productVersion"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }
}

#[async_trait]
impl SystemInfoProvider for MacOsSystemInfo {
    async fn get_info(&self) -> Result<SystemInfo> {
        tokio::task::spawn_blocking(|| {
            let (memory_total, memory_available) = Self::get_memory_info();

            Ok(SystemInfo {
                os_name: "macOS".to_string(),
                os_version: Self::get_os_version(),
                hostname: hostname::get()
                    .map(|h| h.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "unknown".to_string()),
                username: whoami::username(),
                home_dir: dirs::home_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "~".to_string()),
                cpu_arch: std::env::consts::ARCH.to_string(),
                memory_total,
                memory_available,
            })
        })
        .await
        .map_err(|e| AetherError::IoError(format!("Task join error: {}", e)))?
    }

    async fn active_application(&self) -> Result<String> {
        tokio::task::spawn_blocking(|| {
            // Use AppleScript to get frontmost application
            let output = Command::new("osascript")
                .args([
                    "-e",
                    "tell application \"System Events\" to get name of first process whose frontmost is true",
                ])
                .output()
                .map_err(|e| AetherError::IoError(format!("Failed to run osascript: {}", e)))?;

            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Ok("Unknown".to_string())
            }
        })
        .await
        .map_err(|e| AetherError::IoError(format!("Task join error: {}", e)))?
    }

    async fn active_window_title(&self) -> Result<String> {
        tokio::task::spawn_blocking(|| {
            // Use AppleScript to get window title
            // Note: This requires Accessibility permissions
            let output = Command::new("osascript")
                .args([
                    "-e",
                    r#"tell application "System Events"
                        set frontApp to first process whose frontmost is true
                        set windowTitle to ""
                        try
                            set windowTitle to name of front window of frontApp
                        end try
                        return windowTitle
                    end tell"#,
                ])
                .output()
                .map_err(|e| AetherError::IoError(format!("Failed to run osascript: {}", e)))?;

            if output.status.success() {
                let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(if title.is_empty() {
                    "Untitled".to_string()
                } else {
                    title
                })
            } else {
                Ok("Unknown".to_string())
            }
        })
        .await
        .map_err(|e| AetherError::IoError(format!("Task join error: {}", e)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_info() {
        let provider = MacOsSystemInfo::new();
        let info = provider.get_info().await.unwrap();

        assert_eq!(info.os_name, "macOS");
        assert!(!info.os_version.is_empty());
        assert!(!info.hostname.is_empty());
        assert!(!info.username.is_empty());
        assert!(!info.home_dir.is_empty());
        assert!(!info.cpu_arch.is_empty());
        // Memory should be > 0
        assert!(info.memory_total > 0);
    }

    #[tokio::test]
    async fn test_active_application() {
        let provider = MacOsSystemInfo::new();
        // This might not work in CI without proper permissions
        let result = provider.active_application().await;
        assert!(result.is_ok());
    }
}
