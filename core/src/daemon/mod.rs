//! Daemon Manager
//!
//! Manages Aether as a persistent system service across platforms.

pub mod cli;
pub mod error;
pub mod event_bus;
pub mod events;
pub mod ipc;
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use cli::{DaemonCli, DaemonCommand};
pub use error::{DaemonError, Result};
pub use event_bus::DaemonEventBus;
pub use events::{
    DaemonEvent, DerivedEvent, FsEventType, ProcessEventType, RawEvent, SystemEvent,
    SystemStateType, TimeTrigger,
};
pub use ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};

/// Initialize daemon subsystem
pub fn init() -> Result<()> {
    Ok(())
}
