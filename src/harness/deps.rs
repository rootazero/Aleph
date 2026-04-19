//! Harness dependency bundle — assembled once at startup.
//!
//! `HarnessDeps` is the single struct injected into `AgentHarness::new`.
//! All fields are `Arc<dyn Trait>` so the harness is cheaply cloneable and
//! thread-safe.
//!
//! Note: There is no separate `SandboxFactory` trait in this codebase;
//! the factory function (`build_sandbox`) returns `Arc<dyn Sandbox>` directly,
//! so we hold the sandbox instance rather than a factory.

use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;

use std::sync::Arc;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    /// Shared sandbox instance (produced by `build_sandbox` at boot).
    pub sandbox: Arc<dyn Sandbox>,
    pub llm: Arc<dyn AiProvider>,
}
