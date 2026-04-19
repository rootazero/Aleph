//! Sandbox — "where to execute" abstraction, orthogonal to Tools.
//!
//! Exec-class tools hold Arc<dyn Sandbox> and call sandbox.execute(cmd)
//! instead of Command::new(...). WorkspaceSandbox provisions
//! ~/.aleph/workspaces/{session_id}/ lazily and drives macOS seatbelt
//! through OsSandboxDriver.
//!
//! See: docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md

use async_trait::async_trait;

pub mod capabilities;
pub mod command;
pub mod context;
pub mod driver;
pub mod workspace;

pub use capabilities::{NetworkPolicy, SandboxCapabilities};
pub use command::{SandboxCommand, SandboxError, SandboxOutput};
pub use context::current_session;
pub use driver::{OsSandboxDriverTrait, OsSandboxProfile};

#[async_trait]
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(
        &self,
        command: SandboxCommand,
    ) -> Result<SandboxOutput, SandboxError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Minimal stub that proves the Sandbox trait is object-safe and usable
    /// behind `Arc<dyn Sandbox>`. Task 6 replaces this with WorkspaceSandbox.
    struct NoopSandbox;

    #[async_trait]
    impl Sandbox for NoopSandbox {
        async fn execute(
            &self,
            _command: SandboxCommand,
        ) -> Result<SandboxOutput, SandboxError> {
            Err(SandboxError::Other("NoopSandbox".into()))
        }
    }

    #[test]
    fn sandbox_trait_is_object_safe() {
        let _sandbox: Arc<dyn Sandbox> = Arc::new(NoopSandbox);
    }
}
