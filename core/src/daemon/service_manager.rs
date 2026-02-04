use crate::daemon::{DaemonConfig, DaemonError, DaemonStatus, Result, ServiceStatus};
use async_trait::async_trait;

/// Cross-platform service management interface
#[async_trait]
pub trait ServiceManager: Send + Sync {
    /// Install the daemon as a system service
    async fn install(&self, config: &DaemonConfig) -> Result<()>;

    /// Uninstall the daemon service
    async fn uninstall(&self) -> Result<()>;

    /// Start the daemon service
    async fn start(&self) -> Result<()>;

    /// Stop the daemon service
    async fn stop(&self) -> Result<()>;

    /// Get current daemon runtime status
    async fn status(&self) -> Result<DaemonStatus>;

    /// Get service installation status
    async fn service_status(&self) -> Result<ServiceStatus>;
}

/// Create platform-specific service manager
pub fn create_service_manager() -> Result<Box<dyn ServiceManager>> {
    #[cfg(target_os = "macos")]
    {
        use super::platforms::launchd::LaunchdService;
        Ok(Box::new(LaunchdService::new()))
    }

    #[cfg(target_os = "linux")]
    {
        Err(DaemonError::ServiceError("Linux support not yet implemented".to_string()))
    }

    #[cfg(target_os = "windows")]
    {
        Err(DaemonError::ServiceError("Windows support not yet implemented".to_string()))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(DaemonError::ServiceError("Unsupported platform".to_string()))
    }
}
