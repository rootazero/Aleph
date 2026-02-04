//! Daemon Manager
//!
//! Manages Aether as a persistent system service across platforms.

pub mod error;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};

/// Initialize daemon subsystem
pub fn init() -> Result<()> {
    Ok(())
}
