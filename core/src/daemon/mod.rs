//! Daemon Manager
//!
//! Manages Aleph as a persistent system service across platforms.

pub mod cli;
pub mod dispatcher;
pub mod error;
pub mod event_bus;
pub mod events;
pub mod ipc;
pub mod resource_governor;
pub mod service_manager;
pub mod types;
pub mod worldmodel;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use cli::{DaemonCli, DaemonCommand};
pub use error::{DaemonError, Result};
pub use event_bus::DaemonEventBus;
pub use dispatcher::{
    ActionExecutor, ActionType, Dispatcher, DispatcherConfig, DispatcherMode, NotificationPriority,
    Policy, PolicyEngine, ProposedAction, RiskLevel,
};
pub use events::{
    DaemonEvent, DerivedEvent, FsEventType, PressureLevel, PressureType, ProcessEventType,
    RawEvent, SystemEvent, SystemStateType, TimeTrigger,
};
#[cfg(unix)]
pub use ipc::IpcServer;
pub use ipc::{JsonRpcRequest, JsonRpcResponse};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
pub use worldmodel::{
    ActivityType, CoreState, EnhancedContext, InferenceCache, PendingAction, WorldModelConfig,
};

/// Initialize daemon subsystem
pub fn init() -> Result<()> {
    Ok(())
}
