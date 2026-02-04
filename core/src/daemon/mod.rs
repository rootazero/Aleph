//! Daemon Manager
//!
//! Manages Aether as a persistent system service across platforms.

pub mod error;
pub mod types;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};

/// Initialize daemon subsystem
pub fn init() -> Result<()> {
    Ok(())
}
