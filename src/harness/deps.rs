//! Harness dependency bundle — assembled once at startup.
//!
//! `HarnessDeps` is the single struct injected into `AgentHarness::new`.
//! All fields are `Arc<dyn Trait>` so the harness is cheaply cloneable and
//! thread-safe.

use crate::context::budget::preflight::PreflightPipeline;
use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
use crate::harness::trace_sink::TraceSink;
use crate::providers::AiProvider;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    /// The LLM provider. Provider-tier failover (ordered chain, model-level
    /// fallback, circuit breaker) is layered *inside* this `AiProvider` via
    /// `providers::FailoverProvider`; the harness sees one provider and never
    /// knows failover exists (R10 — the loop carries no recovery strategy).
    pub llm: Arc<dyn AiProvider>,

    /// Per-model robustness profile for the active run's model. Resolved by
    /// `harness_bridge::runner_impl` (where the model is known) and read by
    /// `run_verifiers` when building `TurnVerifyContext`. Defaults to
    /// `conservative()` at construction sites that don't resolve a model.
    pub robustness_profile: crate::verification::ModelRobustnessProfile,
    /// Stage 6a seam (#10). Per-turn verifier chain consulted between Think
    /// and Act every iteration. `StopHookVerifier` (in the chain) preserves
    /// pre-6a "veto before Done" semantics; `ToolLoopVerifier` adds `tool_use`
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
    /// Cheap-pass preflight pipeline (`tool_result` pruning + historical
    /// image stripping) that runs BEFORE the budget check + compactor so
    /// token savings happen even when the compactor's LLM call fails.
    /// `None` when `[context_budget]` is disabled — same gating as
    /// `context_compactor`. See `src/context/budget/cheap_passes/`.
    pub preflight_pipeline: Option<Arc<PreflightPipeline>>,
    /// Gateway-side observability sink. `None` falls back to no-op tracing.
    /// Production path: Gateway wraps its persistence callback in `GatewayTraceSink`.
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    /// System prompt injected into every `RequestPayload`. Subagent path builds
    /// this via `PromptBuilder` at spawn time; Gateway passes None for now.
    pub system_prompt: Option<String>,
    /// Stable/dynamic split of the system prompt for cache-first providers.
    ///
    /// When present, the harness threads these through to
    /// `RequestPayload::system_blocks`; the Anthropic adapter uses the split
    /// to place the prompt-cache breakpoint at the stable/dynamic boundary,
    /// so per-turn dynamic content (`RuntimeContext.current_time`,
    /// `tool_runtime_state`, etc.) no longer busts the prefix hash. `None`
    /// keeps the legacy "single-string system" path. These parts are threaded
    /// via `RequestPayload::with_system_blocks` *independently* of
    /// `system_prompt` (see `build_request_payload`) — the harness does NOT
    /// concatenate them into the legacy `system_prompt` field. A caller that
    /// sets parts WITHOUT `system_prompt` leaves the legacy field empty, so an
    /// adapter reading only `system_prompt` (not `system_blocks`) would see no
    /// system prompt; set both if you need that. The production bridge always
    /// sets both, so this never bites today.
    pub system_prompt_parts: Option<Vec<crate::thinker::prompt_builder::SystemPromptPart>>,
    /// Per-run recall context (hybrid memory retrieval + routing experience),
    /// rendered once pre-loop by the harness bridge and appended by `think`
    /// as a transient trailing user message — never persisted, never in the
    /// system prompt. Recall varies per user query, and any varying bytes in
    /// the system prompt sit ahead of every message-level prompt-cache
    /// breakpoint, re-keying the whole conversation prefix each run; at the
    /// message tail a changed recall costs only itself (codex initial-context
    /// / hermes frozen-prompt parity). `None` → no extra message.
    pub recall_context: Option<String>,
    /// Stage 5a seam (#9). Optional registry consulted at three callsites
    /// in `AgentHarness::run_turn_internal`: turn entry (input), model
    /// output emit (output), and tool dispatch (tool-call, Stage 5b).
    /// `None` is equivalent to "no guardrails registered" — zero-cost
    /// noop path with no allocations on the steady-state hot loop.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    /// Hard iteration cap. When set, `AgentHarness::run` forces `TurnState::Done`
    /// after that many Continue turns and sets `hit_limit=true`. None → unbounded
    /// (current Gateway default).
    pub max_iterations: Option<usize>,
    /// Optional power-management capability. When present, the harness inhibits
    /// idle sleep for the duration of each turn so long-running Think→Act loops
    /// don't get cut off by macOS putting the host to sleep.
    pub power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>>,
    /// Stall detection configuration. When set, the harness monitors for
    /// inactivity and, on timeout, sets `TerminateReason::StallTimeout` +
    /// `hit_limit=true` and exits cleanly (not via `HarnessError`).
    pub stall_config: Option<StallConfig>,
    /// Hard cap on consecutive turns where every tool call failed. When
    /// reached, the harness forces `TurnState::Done` with `hit_limit=true`
    /// to prevent the model from looping on permanently-failing tools.
    /// `None` disables the cap (legacy behavior). Recommended `Some(8)`.
    pub consecutive_failure_cap: Option<usize>,
    /// Hard wall-clock budget for a single **Think** phase — an LLM call, which
    /// has no human gate in front of it. Overrunning it yields
    /// `HarnessError::StalledTurn`. It deliberately does NOT bound Act: a tool
    /// call's clock belongs to the tool layer, below the approval gate, or the
    /// operator's reading time is billed to the tool and can abort the run.
    /// `None` disables. Recommended `Some(300s)`.
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
    /// Registrar that makes a split-created child epoch visible to gateway
    /// epoch resolution. `None` disables session-split — the loop falls back
    /// from `SplitSession` to `CompactToFit`'s deterministic truncation floor
    /// when the budget asks for a split.
    pub session_epoch_registrar:
        Option<Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>>,
    /// Spec 3 — fire-and-forget sink for per-tool-invocation signals. Each
    /// completed tool call (success or failure) is forwarded here so the
    /// Dream cycle's signal collector can read cross-session statistics
    /// from `raw_memories`. Defaults to `NoopToolSignalSink` (zero cost)
    /// when no `RawMemoryStore` is wired (tests, simple-engine fixtures).
    pub tool_signal_sink: Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>,
    /// Gap B follow-up — in-flight tool-call registry. When wired, the Act
    /// phase registers each call's per-call `CancellationToken` under its
    /// `tool_call_id` so the gateway's `tools.cancel_call` RPC can fire a
    /// single call's cooperative cancel without aborting the whole run.
    /// `None` disables the wiring; everything still works because the harness
    /// just doesn't expose the in-flight tokens externally.
    pub in_flight_tool_calls: Option<Arc<crate::tools::in_flight::InFlightToolCalls>>,
    /// Maximum number of concurrent-safe tool calls dispatched in parallel
    /// inside a single Act batch.
    ///
    /// When `Some(n)` and `n >= 2`, the Act phase groups concurrent-safe
    /// runs in the batch and `join_all`s up to `n` of them at once,
    /// preserving input-order side effects (event emit, trace, budget,
    /// timeline). Tools that aren't concurrent-safe — or any batch that
    /// has even one unsafe call — fall through to the serial loop
    /// unchanged.
    ///
    /// `None` or `Some(0..=1)` disables the fast path (every batch runs
    /// serially). Recommended: `Some(8)` for production gateway,
    /// matching opencode's `Effect.forEach({ concurrency: 10 })` default.
    pub parallel_tool_concurrency: Option<usize>,
}

// ---------------------------------------------------------------------------
// Stall detection types (consolidated from stall.rs)
// ---------------------------------------------------------------------------

const DEFAULT_STALL_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct StallConfig {
    pub timeout: Duration,
}

impl Default for StallConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_STALL_TIMEOUT_SECS),
        }
    }
}

impl StallConfig {
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug)]
pub(crate) struct StallTracker {
    last_activity: Arc<Mutex<Instant>>,
    config: StallConfig,
}

impl StallTracker {
    #[must_use]
    pub(crate) fn new(config: StallConfig) -> Self {
        Self {
            last_activity: Arc::new(Mutex::new(Instant::now())),
            config,
        }
    }

    /// Record activity (async, for use in async context)
    pub(crate) async fn record_activity(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    /// Get elapsed time since last activity (async)
    pub(crate) async fn elapsed(&self) -> Duration {
        self.last_activity.lock().await.elapsed()
    }

    /// Returns `true` when no activity has been recorded within the
    /// configured timeout. Async because it acquires the same
    /// `tokio::sync::Mutex` as `record_activity` / `elapsed`: under lock
    /// contention it waits for the lock rather than reporting a false
    /// "not stalled".
    pub(crate) async fn is_stalled(&self) -> bool {
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
