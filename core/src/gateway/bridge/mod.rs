pub mod bridged_channel;
pub mod desktop_manager;
pub mod supervisor;
mod types;

pub use bridged_channel::BridgedChannel;
pub use desktop_manager::DesktopBridgeManager;
pub use supervisor::{BridgeSupervisor, BridgeSupervisorError, ManagedProcessConfig, ProcessStatus};
pub use types::*;
