//! Harness dependency bundle — assembled once at startup.
//!
//! `HarnessDeps` is the single struct injected into `AgentHarness::new`.
//! All fields are `Arc<dyn Trait>` so the harness is cheaply cloneable and
//! thread-safe.
//!
//! Note: There is no separate `SandboxFactory` trait in this codebase;
//! the factory function (`build_sandbox`) returns `Arc<dyn Sandbox>` directly,
//! so we hold the sandbox instance rather than a factory.

use crate::context::budget::preflight::PreflightPipeline;
use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
use crate::harness::chain_context::ChainContext;
use crate::harness::prompt::PromptBuilder;
use crate::harness::trace_sink::TraceSink;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    /// Shared sandbox instance (produced by `build_sandbox` at boot).
    pub sandbox: Arc<dyn Sandbox>,
    /// The LLM provider. Provider-tier failover (ordered chain, model-level
    /// fallback, circuit breaker) is layered *inside* this `AiProvider` via
    /// `providers::FailoverProvider`; the harness sees one provider and never
    /// knows failover exists (R10 — the loop carries no recovery strategy).
    pub llm: Arc<dyn AiProvider>,

    /// Stage 6a seam (#10). Per-turn verifier chain consulted between Think
    /// and Act every iteration. `StopHookVerifier` (in the chain) preserves
    /// pre-6a "veto before Done" semantics; `ToolLoopVerifier` adds tool_use
    /// death-loop detection. A `Veto` forces a `[verifier veto]` user-message
    /// injection and an extra `Continue` turn so the model can react.
    /// `None` is equivalent to "no verifiers wired" — zero-cost noop path.
    pub verifier_chain: Option<Arc<VerifierChain>>,
    /// Context pressure sensor — evaluated between turns. Critical pressure
    /// surfaces as `FlowOutcome::hit_limit = true` via
    /// `AgentHarness::hit_limit()`.
    pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
    /// LLM-based compactor invoked when the budget directive is
    /// `CompactAndContinue`. Falls back to deterministic truncation on
    /// provider failure (see `ContextCompactor::compact`).
    pub context_compactor: Option<Arc<ContextCompactor>>,
    /// Cheap-pass preflight pipeline (tool_result pruning + historical
    /// image stripping) that runs BEFORE the budget check + compactor so
    /// token savings happen even when the compactor's LLM call fails.
    /// `None` when `[context_budget]` is disabled — same gating as
    /// `context_compactor`. See `src/context/budget/cheap_passes/`.
    pub preflight_pipeline: Option<Arc<PreflightPipeline>>,
    /// Gateway-side observability sink. `None` falls back to no-op tracing.
    /// Production path: Gateway wraps its persistence callback in `GatewayTraceSink`.
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    /// System prompt injected into every RequestPayload. Subagent path builds
    /// this via PromptBuilder at spawn time; Gateway passes None for now.
    pub system_prompt: Option<String>,
    /// Per-turn message assembler. Stage 3 seam (#5). Defaults to
    /// `DefaultPromptBuilder` (byte-equivalent to legacy build_prompt).
    /// Subagent (#11) and JudgeAgent (#10) inject custom builders.
    pub prompt_builder: Arc<dyn PromptBuilder>,
    /// Position of this harness instance in the subagent call chain.
    /// Stage 4 seam (#11). Defaults to a fresh root chain (depth=0). The
    /// subagent spawner overrides this with `parent.chain.child()` so each
    /// nested harness reports its own depth via `AgentHarness::chain_context()`.
    pub chain_context: ChainContext,
    /// Stage 5a seam (#9). Optional registry consulted at three callsites
    /// in `AgentHarness::run_turn_internal`: turn entry (input), model
    /// output emit (output), and tool dispatch (tool-call, Stage 5b).
    /// `None` is equivalent to "no guardrails registered" — zero-cost
    /// noop path with no allocations on the steady-state hot loop.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
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
    /// Layer 3 of the tool-result budget: per-turn aggregate token tracker
    /// that spills overflowing results to disk via `result_store`. `None`
    /// disables Layer 3 entirely; Layer 2 (per-tool cap) still runs inside
    /// `ScopedToolService` if a store is wired there.
    pub turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>>,
    /// Shared `ToolResultStore` used by Layer 3 spill instructions to
    /// persist large outputs to disk. Should match the store injected
    /// into `ScopedToolService` so all persisted markers land in the
    /// same session-scoped directory.
    pub result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
}

// ---------------------------------------------------------------------------
// Stall detection types (consolidated from stall.rs)
// ---------------------------------------------------------------------------

const DEFAULT_STALL_TIMEOUT_SECS: u64 = 300;
const DEFAULT_STALL_CHECK_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct StallConfig {
    pub timeout: Duration,
    pub check_interval: Duration,
}

impl Default for StallConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_STALL_TIMEOUT_SECS),
            check_interval: Duration::from_secs(DEFAULT_STALL_CHECK_INTERVAL_SECS),
        }
    }
}

impl StallConfig {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }
}

#[derive(Debug)]
pub struct StallTracker {
    last_activity: Arc<Mutex<Instant>>,
    config: StallConfig,
}

impl StallTracker {
    pub fn new(config: StallConfig) -> Self {
        Self {
            last_activity: Arc::new(Mutex::new(Instant::now())),
            config,
        }
    }

    /// Record activity (async, for use in async context)
    pub async fn record_activity(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    /// Get elapsed time since last activity (async)
    pub async fn elapsed(&self) -> Duration {
        self.last_activity.lock().await.elapsed()
    }

    /// Returns `true` when no activity has been recorded within the
    /// configured timeout. Async because it acquires the same
    /// `tokio::sync::Mutex` as `record_activity` / `elapsed`: under lock
    /// contention it waits for the lock rather than reporting a false
    /// "not stalled".
    pub async fn is_stalled(&self) -> bool {
        self.last_activity.lock().await.elapsed() > self.config.timeout
    }
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    #[tokio::test]
    async fn test_not_stalled_when_recent_activity() {
        let config = StallConfig::default();
        let tracker = StallTracker::new(config);

        tracker.record_activity().await;

        assert!(!tracker.is_stalled().await);
    }

    #[tokio::test]
    async fn test_stalled_when_timeout_exceeded() {
        let config = StallConfig {
            timeout: Duration::from_millis(10),
            check_interval: Duration::from_millis(1),
        };
        let tracker = StallTracker::new(config);

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(tracker.is_stalled().await);
    }

    #[tokio::test]
    async fn is_stalled_reports_stall_even_when_lock_is_contended() {
        use tokio::sync::Notify;

        let config = StallConfig {
            timeout: Duration::from_millis(10),
            check_interval: Duration::from_millis(1),
        };
        let tracker = StallTracker::new(config);

        // Become stalled: idle well past the 10ms timeout.
        tokio::time::sleep(Duration::from_millis(25)).await;

        // A second task holds the inner `last_activity` lock; it signals via
        // `Notify` once the lock is actually held, so the assertion below is
        // guaranteed to run under genuine contention (no timing guesswork).
        let lock_held = std::sync::Arc::new(Notify::new());
        let holder = {
            let last_activity = tracker.last_activity.clone();
            let lock_held = lock_held.clone();
            tokio::spawn(async move {
                let _guard = last_activity.lock().await;
                lock_held.notify_one();
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        };
        lock_held.notified().await;

        // The old `try_lock` impl returned `false` here (lock unavailable),
        // missing the stall. The async impl waits for the lock, then correctly
        // observes the elapsed time.
        assert!(
            tracker.is_stalled().await,
            "is_stalled must report a real stall even while the lock is held elsewhere",
        );

        holder.await.unwrap();
    }
}
