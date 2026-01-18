//! System Information Module
//!
//! Provides system information queries through the `SystemInfoProvider` trait.
//! The `MacOsSystemInfo` implementation provides macOS-specific system information.

mod macos;

pub use macos::MacOsSystemInfo;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// System information structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Operating system name (e.g., "macOS", "Windows", "Linux")
    pub os_name: String,
    /// Operating system version (e.g., "14.0")
    pub os_version: String,
    /// Hostname
    pub hostname: String,
    /// Current username
    pub username: String,
    /// User's home directory path
    pub home_dir: String,
    /// CPU architecture (e.g., "arm64", "x86_64")
    pub cpu_arch: String,
    /// Total physical memory in bytes
    pub memory_total: u64,
    /// Available physical memory in bytes
    pub memory_available: u64,
}

/// Trait for system information providers
///
/// This trait provides a unified interface for querying system information
/// that can be implemented by different backends (platform-specific, mock for testing).
#[async_trait]
pub trait SystemInfoProvider: Send + Sync {
    /// Get comprehensive system information
    ///
    /// # Returns
    /// SystemInfo structure with all available system information
    async fn get_info(&self) -> Result<SystemInfo>;

    /// Get the name of the frontmost (active) application
    ///
    /// # Returns
    /// Application name or bundle ID
    async fn active_application(&self) -> Result<String>;

    /// Get the title of the active window
    ///
    /// # Returns
    /// Window title string
    async fn active_window_title(&self) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Mock implementation for testing
    pub struct MockSystemInfo {
        pub info: SystemInfo,
        pub active_app: String,
        pub window_title: String,
    }

    impl Default for MockSystemInfo {
        fn default() -> Self {
            Self {
                info: SystemInfo {
                    os_name: "macOS".to_string(),
                    os_version: "14.0".to_string(),
                    hostname: "test-host".to_string(),
                    username: "testuser".to_string(),
                    home_dir: "/Users/testuser".to_string(),
                    cpu_arch: "arm64".to_string(),
                    memory_total: 16 * 1024 * 1024 * 1024, // 16GB
                    memory_available: 8 * 1024 * 1024 * 1024, // 8GB
                },
                active_app: "Finder".to_string(),
                window_title: "Documents".to_string(),
            }
        }
    }

    #[async_trait]
    impl SystemInfoProvider for MockSystemInfo {
        async fn get_info(&self) -> Result<SystemInfo> {
            Ok(self.info.clone())
        }

        async fn active_application(&self) -> Result<String> {
            Ok(self.active_app.clone())
        }

        async fn active_window_title(&self) -> Result<String> {
            Ok(self.window_title.clone())
        }
    }

    #[tokio::test]
    async fn test_mock_system_info() {
        let provider: Arc<dyn SystemInfoProvider> = Arc::new(MockSystemInfo::default());
        let info = provider.get_info().await.unwrap();

        assert_eq!(info.os_name, "macOS");
        assert_eq!(info.cpu_arch, "arm64");
        assert_eq!(info.memory_total, 16 * 1024 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_mock_active_app() {
        let provider: Arc<dyn SystemInfoProvider> = Arc::new(MockSystemInfo::default());
        let app = provider.active_application().await.unwrap();
        assert_eq!(app, "Finder");
    }
}
