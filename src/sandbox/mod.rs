//! Sandbox — "where to execute" abstraction, orthogonal to Tools.
//!
//! Exec-class tools hold Arc<dyn Sandbox> and call sandbox.execute(cmd)
//! instead of `Command::new`(...). `WorkspaceSandbox` provisions
//! ~/.`aleph/workspaces/{session_id`}/ lazily and drives macOS seatbelt
//! through `OsSandboxDriver`.
//!
//! See: docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md

use async_trait::async_trait;

pub mod capabilities;
pub(crate) mod cgroup_v2;
pub mod command;
pub mod command_policy;
pub mod config;
pub mod context;
pub mod denial_logger;
pub mod deny_globs;
pub(crate) mod dns;
pub mod driver;
pub mod exec_approval;
pub mod factory;
pub mod hooks;
pub mod live_tail;
pub mod platforms;
pub mod policy;
pub mod protected_paths;
pub mod proxy;
pub mod rate_limit;
pub mod resource_governor;
pub mod sandbox_init;
pub mod scrub;
pub mod security_kernel_hook;
pub mod summary;
pub mod windows_init;
pub mod workspace;
pub mod worktree;

pub use capabilities::{NetworkPolicy, SandboxCapabilities};
pub use command::{SandboxCommand, SandboxDenialHint, SandboxError, SandboxOutput};
pub use command_policy::{CommandPolicy, CommandPolicyHook};
pub use config::SandboxConfig;
pub use context::{current_justification, current_session};
pub use driver::{OsSandboxDriverTrait, OsSandboxProfile};
pub use factory::{build_sandbox, NoopSandbox};
pub use hooks::{SandboxHookContext, SandboxHookResult, SandboxHooks};
pub use live_tail::{LiveSnapshot, LiveStream, LiveTail};
pub use platforms::create_platform_driver_from_config;

pub use summary::{NetworkState, PolicyTier, SandboxSummary};
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeSandbox};

#[async_trait]
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(&self, command: SandboxCommand) -> Result<SandboxOutput, SandboxError>;

    /// Codex-inspired prompt surfacing — returns the active sandbox posture
    /// for injection into the LLM system prompt. Default `None` keeps
    /// mock / no-op implementations silent. See [`SandboxSummary`].
    fn summary(&self) -> Option<SandboxSummary> {
        None
    }
}

/// Test helpers exposed under `#[cfg(test)]` so unit tests across the crate
/// can stand up lightweight `Arc<dyn Sandbox>` fixtures without wiring the
/// real `WorkspaceSandbox` + OS driver stack. Task 8 consumers use
/// `MockSandbox` to verify exec-class tools route through the sandbox seam.
#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;

    /// Records every `SandboxCommand` it receives and returns a canned
    /// `SandboxOutput`. `calls` is exposed for assertions; `response` is
    /// immutable once constructed.
    pub struct MockSandbox {
        pub calls: Mutex<Vec<SandboxCommand>>,
        pub response: SandboxOutput,
    }

    impl MockSandbox {
        pub fn new(response: SandboxOutput) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                response,
            })
        }
    }

    #[async_trait]
    impl Sandbox for MockSandbox {
        async fn execute(&self, cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
            self.calls.lock().await.push(cmd);
            Ok(self.response.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn sandbox_trait_is_object_safe() {
        // Exercises trait object assembly via the shared NoopSandbox stub
        // exported from `factory`. Task 6 provides the real WorkspaceSandbox.
        let _sandbox: Arc<dyn Sandbox> = Arc::new(NoopSandbox);
    }
}
