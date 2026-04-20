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

use crate::harness::context_budget::ContextBudget;
use crate::harness::context_compactor::ContextCompactor;
use crate::harness::skill_prefetch::SkillPrefetcher;
use crate::harness::stop_hooks::StopHookHandler;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;

use std::sync::Arc;
use tokio::sync::Mutex;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    /// Shared sandbox instance (produced by `build_sandbox` at boot).
    pub sandbox: Arc<dyn Sandbox>,
    pub llm: Arc<dyn AiProvider>,

    // -- Task 10 (6b) wiring -------------------------------------------------
    //
    // Optional collaborators inherited from the retiring `AgentLoop` builder
    // chain. Each defaults to `None`; Task 11 will collapse the marker block
    // above this struct once wiring is observed stable.
    /// Stop hooks consulted before the harness yields `TurnState::Done`.
    /// A blocking verdict forces an extra `Continue` turn so the model can
    /// react to the veto (e.g. "tests are failing, try again").
    pub stop_hooks: Option<Arc<Vec<Arc<dyn StopHookHandler>>>>,
    /// Context pressure sensor — evaluated between turns. Critical pressure
    /// surfaces as `FlowOutcome::hit_limit = true` via
    /// `AgentHarness::hit_limit()`.
    pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
    /// LLM-based compactor invoked when the budget directive is
    /// `CompactAndContinue`. Falls back to deterministic truncation on
    /// provider failure (see `ContextCompactor::compact`).
    pub context_compactor: Option<Arc<ContextCompactor>>,
    /// Optional async skill discovery prefetcher. Wired from the orchestrator
    /// boot path; the harness triggers a throttled scan at the start of each
    /// Think pass so newly available skills are surfaced without adding
    /// latency to the main loop.
    pub skill_prefetcher: Option<Arc<SkillPrefetcher>>,
}
