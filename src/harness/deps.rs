//! Harness dependency bundle — assembled once at startup.
//!
//! `HarnessDeps` is the single struct injected into `AgentHarness::new`.
//! All fields are `Arc<dyn Trait>` so the harness is cheaply cloneable and
//! thread-safe.
//!
//! Note: There is no separate `SandboxFactory` trait in this codebase;
//! the factory function (`build_sandbox`) returns `Arc<dyn Sandbox>` directly,
//! so we hold the sandbox instance rather than a factory.
//!
//! # PHASE-6b-WIRING (deferred from Phase 6a Task 6)
//!
//! Phase 6b MUST add these optional fields and wire them into
//! `AgentHarness::run_turn`:
//!
//! ```ignore
//! pub stop_hooks: Option<Arc<StopHooksExecutor>>,
//! pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
//! pub context_compactor: Option<Arc<ContextCompactor>>,
//! ```
//!
//! Behavioural integration required in `src/harness/agent.rs`:
//!   * stop hooks evaluated before an early-exit / TurnState::Done handoff,
//!   * budget check between iterations populates `FlowOutcome::hit_limit`,
//!   * compactor fires when pressure crosses the configured threshold.
//!
//! The helpers relocate in Phase 6b (agent_loop/context_budget →
//! harness/context_budget, agent_loop/context_compactor.rs →
//! harness/context_compactor.rs, agent_loop/stop_hooks.rs →
//! harness/stop_hooks.rs) — wiring them here at the same time avoids the
//! reverse `harness → agent_loop` dependency that blocked doing this in 6a.
//!
//! See `docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
//! §6 "Inherited from Phase 6a — Task 6 deferred" for the full scope +
//! test requirements. Remove this entire marker block once 6b lands.

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
