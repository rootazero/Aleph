//! Harness dependency bundle — assembled once at startup.
//!
//! `HarnessDeps` is the single struct injected into `AgentHarness::new`.
//! All fields are `Arc<dyn Trait>` so the harness is cheaply cloneable and
//! thread-safe.
//!
//! Note: There is no separate `SandboxFactory` trait in this codebase;
//! the factory function (`build_sandbox`) returns `Arc<dyn Sandbox>` directly,
//! so we hold the sandbox instance rather than a factory.

use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
use crate::harness::stall::StallConfig;
use crate::harness::trace_sink::TraceSink;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::skill::prefetch::SkillPrefetcher;
use crate::tools::service::ToolService;
use crate::verification::stop_hooks::StopHookHandler;

use std::sync::Arc;
use tokio::sync::Mutex;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    /// Shared sandbox instance (produced by `build_sandbox` at boot).
    pub sandbox: Arc<dyn Sandbox>,
    pub llm: Arc<dyn AiProvider>,

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
    /// Gateway-side observability sink. `None` falls back to no-op tracing.
    /// Production path: Gateway wraps its persistence callback in `GatewayTraceSink`.
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    /// System prompt injected into every RequestPayload. Subagent path builds
    /// this via PromptBuilder at spawn time; Gateway passes None for now.
    pub system_prompt: Option<String>,
    /// Hard iteration cap. When set, AgentHarness::run forces TurnState::Done
    /// after that many Continue turns and sets hit_limit=true. None → unbounded
    /// (current Gateway default).
    pub max_iterations: Option<usize>,
    /// Optional power-management capability. When present, the harness inhibits
    /// idle sleep for the duration of each turn so long-running Think→Act loops
    /// don't get cut off by macOS putting the host to sleep.
    pub power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>>,
    /// Stall detection configuration. When set, the harness monitors for
    /// inactivity and returns `HarnessError::Stalled` if no activity
    /// is detected within the configured timeout.
    pub stall_config: Option<StallConfig>,
    /// Hard cap on consecutive turns where every tool call failed. When
    /// reached, the harness forces `TurnState::Done` with `hit_limit=true`
    /// to prevent the model from looping on permanently-failing tools.
    /// `None` disables the cap (legacy behavior). Recommended `Some(8)`.
    pub consecutive_failure_cap: Option<usize>,
    /// Hard wall-clock budget for a single Think or Act phase. When set, the
    /// harness wraps each LLM call and each tool exec in `tokio::time::timeout`.
    /// Exceeding the budget yields `HarnessError::StalledTurn` with the
    /// hung phase. `None` disables (legacy behavior). Recommended `Some(300s)`.
    pub turn_timeout: Option<std::time::Duration>,
}
