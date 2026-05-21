//! Perception subsystem for SnapshotTool and System State Bus.

mod types;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stub;

pub mod state_bus;
pub mod simulation_executor;
pub mod action_dispatcher;
pub mod connectors;
pub mod pal; // Platform Abstraction Layer

pub use types::*;
pub use simulation_executor::SimulationExecutor;
pub use action_dispatcher::{ActionDispatcher, ActionRequest, ActionResult, ActionMethod, ExpectCondition, ConditionType};

use crate::error::Result;

/// Capture a perception snapshot.
pub async fn capture_snapshot(args: SnapshotCaptureArgs) -> Result<PerceptionSnapshot> {
    #[cfg(target_os = "macos")]
    {
        macos::capture_snapshot(args).await
    }

    #[cfg(not(target_os = "macos"))]
    {
        return stub::capture_snapshot(args).await;
    }
}
