//! `AgentHarness` — the concrete Think→Act implementation.
//!
//! Task 8 implemented the Think half of the loop. Task 9 added:
//!   * Act dispatch (executing `tool_calls` sequentially, emitting `ToolResult` /
//!     `ToolError` events).
//!   * Preservation of assistant `tool_use` intent inside `AssistantMessage`
//!     events so later Think cycles can reconstruct the conversation.
//!   * Full-history prompt assembly (now in `agent/prompt.rs`) that re-emits
//!     the preceding assistant `tool_use` turn and resolves real tool names
//!     for `ToolResult` messages.
//!
//! Task 10 (Phase 6b) additionally consumes the optional triad on
//! `HarnessDeps`:
//!   * `context_budget.before_turn(...)` — drives compaction / `hit_limit`.
//!   * `context_compactor.compact(...)` — fires when budget directs warning.
//!   * `verifier_chain` — consulted between Think and Act every turn;
//!     a blocking verdict forces one more `Continue` so the model reacts.
//!
//! Split into sub-modules:
//!   * `think.rs` — `run_turn_internal`, `race_llm_call`, `run_verifiers`
//!   * `act.rs` — `act`
//!   * `guardrails.rs` — `apply_input_guardrail`, `apply_tool_call_guardrail`
//!   * `prompt.rs` — `build_prompt` (sync per-turn message assembler)

use crate::sync_primitives::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Mutex, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::harness::callback::{HarnessCallback, NoopHarnessCallback};
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState, TurnStep};
use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};
use crate::providers::adapter::NativeToolCall;

use crate::session::events::{MessageContent, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::SessionId;
use crate::verification::{ToolCallSummary, TOOL_HISTORY_WINDOW};

mod act;
mod guardrails;
pub(crate) mod prompt;
mod think;

/// Dedicated short budget for the timeout-salvage grace turn. A per-turn /
/// stall timeout means a step is likely slow or hung, so the salvage call must
/// not itself wait another full `turn_timeout`. Fail-soft on expiry.
const GRACE_TIMEOUT_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// Soft-landing reminder injected one turn before the consecutive-failure cap
/// fires. Gives a weak model a final chance to change approach or wrap up
/// before the hard stop. The model writes the user-facing text (R7).
const SOFT_FAILURE_WARNING: &str = "<system-reminder>\nRepeated tool failures \
detected. You are one step from the safety cap stopping this run. Either change \
your approach now (different tool, arguments, or strategy), or stop calling \
tools and summarize for the user what you attempted and what is blocking you.\n\
</system-reminder>";

/// A "failure turn" for the consecutive-failure watchdog: the turn made no net
/// progress because failures outnumber successes. Pure structural count — no
/// judgment about whether a *successful* call was actually useful (R10-safe).
const fn is_failure_turn(executed: usize, errors: usize) -> bool {
    errors > executed
}

/// A "clean turn" resets the streak: zero tool errors this turn. An interleaved
/// single success no longer zeroes a churning failure streak — only a fully
/// clean turn does.
const fn is_clean_turn(_executed: usize, errors: usize) -> bool {
    errors == 0
}

/// Outcome of `AgentHarness::apply_input_guardrail`. The two non-block
/// variants both carry the (possibly mutated) events vector; the caller
/// rebinds `events` to the returned vector before assembling the prompt.
pub(crate) enum InputGuardrailOutcome {
    /// Pass-through; events are unchanged.
    Allow(Vec<crate::session::events::SessionEventRecord>),
    /// Latest `UserMessage`'s text was rewritten in-memory only — the
    /// session log retains the original event for audit.
    Sanitized(Vec<crate::session::events::SessionEventRecord>),
    /// Guardrail blocked the turn; caller emits `on_safety_block` and
    /// returns `TurnState::Done` without invoking the LLM.
    Blocked(String),
}

/// Stage 5b tool-call guardrail outcome. `Block` means the helper already
/// fired `on_safety_block` + emitted `ToolError`; the caller `continue`s.
pub(crate) enum ToolCallGuardOutcome {
    Pass,
    Sanitize(Value),
    Block,
}

pub struct AgentHarness {
    pub(super) deps: HarnessDeps,
    /// Tracks agent activity for stall detection. `None` when stall detection
    /// is disabled (no `stall_config` in deps).
    pub(super) stall_tracker: Option<crate::harness::deps::StallTracker>,
    /// Set when `context_budget.before_turn` returns `FinalReply`. Surfaced
    /// through [`AgentHarness::hit_limit`] so the orchestrator bridge can
    /// populate `FlowOutcome::hit_limit`.
    hit_limit: AtomicBool,
    /// Cumulative provider-reported token usage across every LLM call in
    /// this run (`input + output + cache_read + cache_creation`). Read after
    /// the run via [`AgentHarness::total_tokens`] by the orchestrator bridge
    /// and subagent spawner. A harness instance serves a single run, so the
    /// counter is never reset.
    total_tokens: AtomicU64,
    /// Precise loop-exit cause. Defaults to `Completed` and is overwritten
    /// at each cap site inside `run()`. Granular complement to
    /// [`hit_limit`]; read after the run via [`AgentHarness::terminate_reason`].
    /// One run per harness instance — never reset.
    terminate_reason: Mutex<TerminateReason>,
    /// One push per `ToolCallCompleted` trace event so the timeline carries
    /// the same metadata trace consumers see (name, duration, success). Read
    /// after the run via [`AgentHarness::tool_timeline`].
    tool_timeline: Mutex<Vec<ToolInvocation>>,
    /// Per-component token breakdown accumulated alongside [`total_tokens`].
    /// Each successful LLM call in `think.rs` folds its `TokenUsage` here
    /// via [`TokenBreakdown::accumulate`]. Read after the run via
    /// [`AgentHarness::token_breakdown`].
    token_breakdown: Mutex<TokenBreakdown>,
    /// Wall-clock start of `run()`. Read after the run via
    /// [`AgentHarness::duration_ms`] to derive elapsed milliseconds.
    started_at: OnceLock<Instant>,
    /// The session id at the end of `run()`. Equals the parent id on a normal
    /// run; equals the child id when a `SplitSession` directive caused the
    /// loop to switch sessions mid-run. `None` until `run()` completes.
    final_session_id: Mutex<Option<SessionId>>,
    /// Cross-batch dedup: `(tool_name, canonical_args)` signatures whose last
    /// attempt in this run produced a `ToolError`. Consulted by `act()` /
    /// `act_parallel()` at preflight — identical repeats are rejected with a
    /// synthetic `ToolError::Execution` instead of re-running. Cleared
    /// whenever any tool call in a turn succeeds (LLM has made progress).
    /// Prevents the pathological loop where the model keeps re-issuing the
    /// same deterministically-failing call (e.g. `web_fetch` against a
    /// sandbox-blocked URL or a quota-exhausted API).
    pub(super) recent_failures: Mutex<std::collections::HashSet<(String, String)>>,
    /// Counter for reactive-compaction rescue attempts (claude-code parity,
    /// query.ts:1092). When a provider returns `prompt_too_long` / 413, the
    /// harness consults [`crate::providers::llm_retry::classify`]; on the
    /// `CompactAndRetry` verdict it calls `context_compactor` and retries
    /// the LLM call ONCE per run. The atomic is bumped *before* the rescue
    /// attempt so concurrent paths cannot race past the cap. Capped at
    /// `MAX_REACTIVE_COMPACT_ATTEMPTS` (1) — repeated overflows after
    /// summarisation indicate fundamentally oversized input rather than a
    /// recoverable burst. R10-safe: pure round scheduling, no policy choice.
    pub(super) reactive_compact_attempts: AtomicU32,
    /// Persisted event-log length captured at the start of the most recent turn
    /// (before that turn appended anything), i.e. the index boundary the turn's
    /// prompt was built from. The follow-up boundary check
    /// ([`AgentHarness::has_unanswered_user_message`]) treats any non-synthetic
    /// `UserMessage` at or beyond this watermark as input the model never saw.
    ///
    /// Why a watermark and not "user after last assistant": a steering message
    /// injected *while the final turn's LLM call is still streaming* lands in the
    /// log *before* the assistant message that turn commits, so the positional
    /// "is there a user after the last assistant" test silently misses it. The
    /// watermark is set once per turn in `think.rs` (right after `get_events`),
    /// so on a `Done` turn it holds that turn's pre-prompt boundary — late
    /// arrivals sit at index >= watermark regardless of the assistant's position.
    /// Mechanical (R10-safe): one store per turn, no decision. The loop runs one
    /// turn at a time, so `Relaxed` ordering is sufficient.
    pub(super) last_prompt_log_len: AtomicUsize,
}

impl AgentHarness {
    #[must_use]
    pub fn new(deps: HarnessDeps) -> Self {
        let stall_tracker = deps
            .stall_config
            .as_ref()
            .map(|config| crate::harness::deps::StallTracker::new(config.clone()));
        Self {
            deps,
            stall_tracker,
            hit_limit: AtomicBool::new(false),
            total_tokens: AtomicU64::new(0),
            terminate_reason: Mutex::new(TerminateReason::Completed),
            tool_timeline: Mutex::new(Vec::new()),
            token_breakdown: Mutex::new(TokenBreakdown::default()),
            started_at: OnceLock::new(),
            final_session_id: Mutex::new(None),
            recent_failures: Mutex::new(std::collections::HashSet::new()),
            reactive_compact_attempts: AtomicU32::new(0),
            last_prompt_log_len: AtomicUsize::new(0),
        }
    }

    /// Atomically reserve one reactive-compaction rescue slot. Returns
    /// `true` exactly once per run (the cap is
    /// `MAX_REACTIVE_COMPACT_ATTEMPTS = 1`); subsequent callers see `false`
    /// and must surface the original provider error. `compare_exchange` so
    /// concurrent paths cannot both reserve under high concurrency.
    pub(super) fn try_reserve_reactive_compact(&self) -> bool {
        self.reactive_compact_attempts
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Test-only: read the rescue counter without mutating it. Returns the
    /// number of attempts reserved by `try_reserve_reactive_compact` (0 or
    /// 1 in the current cap regime).
    #[cfg(test)]
    pub(super) fn reactive_compact_attempts_for_tests(&self) -> u32 {
        self.reactive_compact_attempts.load(Ordering::Acquire)
    }

    /// Cross-batch dedup probe. `true` when an identical `(tool_name,
    /// canonical_args)` already failed earlier in this run AND no later
    /// success has cleared the set.
    pub(super) fn is_recent_failure(&self, name: &str, canonical_args: &str) -> bool {
        self.recent_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(name.to_string(), canonical_args.to_string()))
    }

    /// Record a `(tool, args)` that just failed. Subsequent identical calls
    /// in later turns will be preflight-rejected.
    pub(super) fn record_failure(&self, name: String, canonical_args: String) {
        self.recent_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((name, canonical_args));
    }

    /// Clear the failure set. Called whenever any tool call in a turn
    /// succeeds — successful progress is the LLM's signal that it has
    /// pivoted, and we don't want to keep blocking historical failures
    /// indefinitely.
    pub(super) fn clear_failures(&self) {
        self.recent_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// `true` if a budget directive forced an early exit during this run.
    /// A harness instance serves exactly one run, so this flag is never reset.
    pub fn hit_limit(&self) -> bool {
        self.hit_limit.load(Ordering::Relaxed)
    }

    /// The session id at the end of the most recent `run()`. `None` when
    /// `run()` has not been called or the harness is still running.
    /// Equals the parent id on a normal run; equals the child id when
    /// a `SplitSession` directive switched the loop to a child session.
    pub fn final_session_id(&self) -> Option<SessionId> {
        self.final_session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Cumulative provider-reported token usage observed across every LLM
    /// call in this run. Components summed: `input + output + cache_read +
    /// cache_creation` (see `turn_token_total`). A harness instance serves
    /// exactly one run, so this counter is never reset.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens.load(Ordering::Relaxed)
    }

    /// Precise loop-exit cause for the most recent run. Defaults to
    /// `Completed` for a clean exit. Set at each cap site inside `run()`.
    pub fn terminate_reason(&self) -> TerminateReason {
        self.terminate_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Per-component token breakdown. Granular complement to
    /// [`total_tokens`] — surfaces cache hit ratio and reasoning-token
    /// spend that the single sum hides.
    pub fn token_breakdown(&self) -> TokenBreakdown {
        self.token_breakdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Tool invocation timeline in run order. Sourced from the same trace
    /// emissions the harness sends to `ToolCallCompleted`, so trace
    /// consumers and the `FlowOutcome` see the same metadata.
    pub fn tool_timeline(&self) -> Vec<ToolInvocation> {
        self.tool_timeline
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Wall-clock duration since `run()` entry. Returns `0` when the loop
    /// has not started (test fixtures that call accessors without driving
    /// the loop).
    pub fn duration_ms(&self) -> u64 {
        self.started_at.get().map_or(0, |t| {
            t.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
        })
    }

    /// Helper for `act.rs` — append one tool invocation to the timeline.
    /// `error` is `Some` iff `success == false`.
    ///
    /// Also fires a best-effort, fire-and-forget call to
    /// `deps.tool_signal_sink` so Dream-cycle metrics can read cross-session
    /// tool statistics from `raw_memories`. The sink lives in `tokio::spawn`
    /// so a slow store cannot back-pressure the harness loop. Production
    /// wiring sets `RawMemoryToolSink`; tests get `NoopToolSignalSink`.
    pub(crate) fn push_tool_invocation(
        &self,
        id: String,
        name: String,
        duration_ms: u64,
        success: bool,
        error: Option<String>,
    ) {
        let mut guard = self.tool_timeline.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(ToolInvocation {
            id,
            name: name.clone(),
            duration_ms,
            success,
            error: error.clone(),
        });
        drop(guard);

        let sink = self.deps.tool_signal_sink.clone();
        tokio::spawn(async move {
            sink.record_tool_call(&name, success, duration_ms, error.as_deref())
                .await;
        });
    }

    /// Set the `TerminateReason` for the run. Called at each cap site in
    /// `run()` (stall, turn timeout, consecutive-failure, verifier-veto,
    /// max-iter) and at cancellation. Last writer wins — the loop is
    /// single-threaded and only writes once per run anyway.
    pub(crate) fn set_terminate_reason(&self, reason: TerminateReason) {
        *self
            .terminate_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = reason;
    }

    /// Fold one provider `TokenUsage` into the accumulated breakdown.
    /// Called from `think.rs` alongside the existing `total_tokens`
    /// `fetch_add` so the two counters stay in lockstep.
    pub(crate) fn accumulate_token_breakdown(
        &self,
        usage: &Option<crate::providers::adapter::TokenUsage>,
    ) {
        if let Some(u) = usage {
            self.token_breakdown
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .accumulate(
                    u.input_tokens,
                    u.output_tokens,
                    u.cache_read_tokens,
                    u.cache_creation_tokens,
                    u.thinking_tokens,
                );
        }
    }

    /// Read-only accessor for this harness's position in the subagent chain.
    /// Returns the root context for top-level agents (the `HarnessDeps`
    /// default). The subagent spawner overrides this with the descended
    /// chain when assembling a child harness. Stage 4 seam (#11).
    pub const fn chain_context(&self) -> &crate::harness::chain_context::ChainContext {
        &self.deps.chain_context
    }

    /// Max times a `Done` turn may be overridden because a steering message
    /// landed during the run's final turn (Pi `getFollowUpMessages` parity).
    /// Bounds a pathological appender that keeps writing user messages so the
    /// loop can never spin forever on follow-up continuations alone; the
    /// normal stop/iteration caps still apply on top of this.
    const MAX_FOLLOWUP_CONTINUATIONS: usize = 8;

    /// Lazy-construct a `LoopTraceEvent` and forward to `trace_sink`.
    /// Returns immediately when no sink is wired — the closure is not invoked.
    pub(crate) fn emit<F>(&self, build: F)
    where
        F: FnOnce() -> crate::harness::trace::LoopTraceEvent,
    {
        if let Some(ref sink) = self.deps.trace_sink {
            sink.on_trace(&build());
        }
    }

    /// Pi `getFollowUpMessages` parity — detect a steering message that landed
    /// during the run's *final* turn and is therefore still unanswered.
    ///
    /// Returns `true` iff a non-synthetic [`SessionEvent::UserMessage`] appears
    /// *after* the last [`SessionEvent::AssistantMessage`] in the session log.
    /// That ordering is the entire signal: the model produced its final turn,
    /// then the user spoke — so the model has not yet seen, let alone answered,
    /// that input. The mid-loop steering path
    /// ([`crate::gateway::execution_engine`]) documents exactly this race: an
    /// injection that lands during the closing LLM call would otherwise sit at
    /// the tail of the log until the user sent a *second* message. Continuing
    /// the loop here answers it in the same run.
    ///
    /// When the newest user message *precedes* the last assistant turn, the
    /// model already saw it and chose to stop; that decision is the model's
    /// (R7), so we return `false` and let the loop terminate. A read error also
    /// returns `false` — failing closed keeps a transient session-store glitch
    /// from spinning the loop. Purely positional: no intent, completion, or
    /// relevance judgement is made (R10-safe scaffolding).
    async fn has_unanswered_user_message(&self, session: &SessionId) -> bool {
        let events = match self.deps.session.get_events(session, None, None).await {
            Ok(e) => e,
            Err(_) => return false,
        };
        // A follow-up only makes sense once at least one assistant turn has run;
        // before that the leading prompt is the turn being processed, not a late
        // arrival. (Also guards the cold default watermark of 0.)
        if !events
            .iter()
            .any(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        {
            return false;
        }
        // Any non-synthetic UserMessage at or beyond the last turn's prompt
        // boundary arrived after that turn read the log, so the model never saw
        // it — even if it landed *before* the assistant message the turn went on
        // to commit (the during-final-turn injection race). Positional and
        // watermark-bounded; R10-safe.
        let watermark = self.last_prompt_log_len.load(Ordering::Relaxed);
        events.iter().skip(watermark).any(|r| {
            matches!(
                &r.event,
                SessionEvent::UserMessage { synthetic, .. } if !*synthetic
            )
        })
    }
}

#[async_trait]
impl crate::session::SessionDriver for AgentHarness {
    async fn drive(&self, session_id: &SessionId) -> crate::error::Result<()> {
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run(session_id, &mut cb, &cancel)
            .await
            .map_err(|e| match e {
                HarnessError::Cancelled => crate::error::AlephError::Cancelled,
                HarnessError::Llm(inner) => inner,
                HarnessError::Tool(tool_err) => {
                    crate::error::AlephError::provider(format!("harness tool error: {tool_err}"))
                }
                HarnessError::Session(sess_err) => {
                    crate::error::AlephError::provider(format!("harness session error: {sess_err}"))
                }
                HarnessError::StalledTurn { phase, elapsed } => crate::error::AlephError::provider(
                    format!("agent turn stalled in {phase} after {elapsed:?}"),
                ),
            })
    }
}

#[async_trait]
impl Harness for AgentHarness {
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        Some(self.chain_context())
    }

    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        // Stamp run start so `duration_ms()` is meaningful. `set` is a no-op
        // when the same harness is reused — single-run instances see one
        // entry only.
        let _ = self.started_at.set(Instant::now());
        // Track which session the loop is currently driving. Starts as the
        // caller's session; rebinds to the child session when a SplitSession
        // directive succeeds. Stored in `final_session_id` at loop exit.
        // Split-safety: StallTracker is time-based (Instant) with no session id — split-safe.
        // TraceSink receives LoopTraceEvent payloads that carry no session id field — split-safe.
        let mut current_session: SessionId = session_id.clone();
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut verifier_veto_count: usize = 0;
        let mut consecutive_failure_turns: usize = 0;
        // Pi `getFollowUpMessages` parity: how many times a `Done` turn has been
        // overridden because a steering message arrived after the model's final
        // turn. Bounded by `MAX_FOLLOWUP_CONTINUATIONS`.
        let mut followup_continuations: usize = 0;
        let mut tool_history: std::collections::VecDeque<ToolCallSummary> =
            std::collections::VecDeque::with_capacity(TOOL_HISTORY_WINDOW);
        let result: Result<crate::harness::trace::LoopTraceSessionOutcome, HarnessError> = loop {
            if cancel.is_cancelled() {
                self.set_terminate_reason(TerminateReason::Cancelled);
                break Err(HarnessError::Cancelled);
            }
            if let Some(ref tracker) = self.stall_tracker {
                if tracker.is_stalled().await {
                    let elapsed = tracker.elapsed().await;
                    tracing::warn!(
                        ?current_session,
                        ?elapsed,
                        "stall watchdog tripped; forcing Done with hit_limit",
                    );
                    self.hit_limit.store(true, Ordering::Relaxed);
                    self.set_terminate_reason(TerminateReason::StallTimeout {
                        elapsed_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    });
                    let _ = tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await;
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                }
            }
            match self
                .run_turn_internal(
                    &current_session,
                    callback,
                    iterations,
                    tool_calls_made,
                    &mut tool_history,
                    cancel,
                )
                .await
            {
                Err(HarnessError::StalledTurn { phase, elapsed }) => {
                    tracing::warn!(
                        ?current_session,
                        ?phase,
                        ?elapsed,
                        "per-turn timeout tripped; forcing Done with hit_limit",
                    );
                    self.hit_limit.store(true, Ordering::Relaxed);
                    self.set_terminate_reason(TerminateReason::TurnTimeout {
                        phase: phase.to_string(),
                        elapsed_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    });
                    let _ = tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await;
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                }
                Err(e) => {
                    // Any other error path collapses to "Cancelled" for the
                    // outer outcome (see Err branch below). Recording it here
                    // keeps `terminate_reason` consistent with the trace.
                    self.set_terminate_reason(TerminateReason::Cancelled);
                    break Err(e);
                }
                Ok(TurnStep {
                    state: TurnState::Continue,
                    executed,
                    vetoed,
                    split_child,
                }) => {
                    if let Some(child) = split_child {
                        current_session = child;
                    }
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    // Consecutive-failure watchdog (structural). Count this
                    // turn's tool errors vs successful executions over the
                    // events since the last assistant message.
                    if !vetoed {
                        let events = self
                            .deps
                            .session
                            .get_events(&current_session, None, None)
                            .await
                            .map_err(HarnessError::Session)?;
                        let last_assistant_idx = events
                            .iter()
                            .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
                            .unwrap_or(0);
                        let errors = events[last_assistant_idx..]
                            .iter()
                            .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
                            .count();
                        if is_clean_turn(executed, errors) {
                            consecutive_failure_turns = 0;
                        } else if is_failure_turn(executed, errors) {
                            consecutive_failure_turns = consecutive_failure_turns.saturating_add(1);
                            if let Some(cap) = self.deps.consecutive_failure_cap {
                                // Soft landing: one turn before the hard cap,
                                // inject a synthetic reminder so the model can
                                // self-correct or wrap up (mirrors the G1
                                // max-steps hint). Structural trigger, R10-safe.
                                if cap > 1 && consecutive_failure_turns == cap - 1 {
                                    let warn = SessionEvent::UserMessage {
                                        turn_id: uuid::Uuid::new_v4(),
                                        content: MessageContent {
                                            text: SOFT_FAILURE_WARNING.to_string(),
                                            blocks: Vec::new(),
                                            thinking: None,
                                            thinking_signature: None,
                                        },
                                        at: crate::session::events::now_ms(),
                                        synthetic: true,
                                    };
                                    if let Err(e) = self
                                        .deps
                                        .session
                                        .emit_event(&current_session, warn)
                                        .await
                                    {
                                        tracing::warn!(?current_session, ?e, "soft-failure warning emit failed");
                                    }
                                }
                                if consecutive_failure_turns >= cap {
                                    tracing::warn!(
                                        ?current_session,
                                        cap,
                                        "consecutive failure cap reached; forcing Done",
                                    );
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    self.set_terminate_reason(
                                        TerminateReason::ConsecutiveFailureCap {
                                            consecutive: consecutive_failure_turns
                                                .try_into()
                                                .unwrap_or(u32::MAX),
                                        },
                                    );
                                    self.fire_boundary_grace_turn(
                                        &current_session,
                                        callback,
                                        iterations,
                                        crate::harness::agent::think::GraceReason::ConsecutiveFailureCap,
                                        cancel,
                                    )
                                    .await;
                                    callback.on_complete();
                                    break Ok(
                                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                                    );
                                }
                            }
                        }
                        // else: minority-failure turn — neither reset nor
                        // increment (hold the streak).
                    }
                    if vetoed {
                        verifier_veto_count = verifier_veto_count.saturating_add(1);
                        if verifier_veto_count >= self.deps.robustness_profile.steer_max {
                            tracing::warn!(
                                ?current_session,
                                max_vetos = self.deps.robustness_profile.steer_max,
                                "verifier veto limit reached; forcing Done to prevent infinite loop",
                            );
                            self.hit_limit.store(true, Ordering::Relaxed);
                            self.set_terminate_reason(TerminateReason::VerifierVeto {
                                vetos: verifier_veto_count.try_into().unwrap_or(u32::MAX),
                            });
                            // Surface a context-rich terminal message instead
                            // of a silent HitLimit break. The remaining steps are
                            // already in the prompt (the `[verifier veto]` events).
                            self.fire_boundary_grace_turn(
                                &current_session,
                                callback,
                                iterations,
                                crate::harness::agent::think::GraceReason::VerifierVeto,
                                cancel,
                            )
                            .await;
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    } else {
                        verifier_veto_count = 0;
                    }
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::Relaxed);
                            self.set_terminate_reason(TerminateReason::HitMaxIterations {
                                used: iterations.try_into().unwrap_or(u32::MAX),
                            });
                            // C1: rescue a runaway that ended on an unresolved
                            // tool_use so the user gets a terminal summary
                            // instead of an empty / mid-thought response.
                            // No-op when the last assistant turn already has
                            // text — well-behaved capped runs pay nothing.
                            self.fire_boundary_grace_turn(
                                &current_session,
                                callback,
                                iterations,
                                crate::harness::agent::think::GraceReason::MaxIterations,
                                cancel,
                            )
                            .await;
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    }
                }
                Ok(TurnStep {
                    state: TurnState::Done,
                    ..
                }) => {
                    // Pi `getFollowUpMessages` parity. A steering message that
                    // landed during this run's final turn was not in that turn's
                    // prompt (the boundary race in
                    // `gateway::execution_engine::steering`). Before handing
                    // control back, re-read the log: if a genuine user message
                    // arrived at or beyond the final turn's prompt boundary
                    // (`last_prompt_log_len`), the model has not seen it —
                    // continue the loop so it is answered in this same run
                    // instead of waiting for the user to send a second message.
                    // The watermark catches messages injected *during* the final
                    // LLM call too, which land *before* the just-committed
                    // assistant message and so escape a naive "user after last
                    // assistant" test. Purely positional (R10-safe) and bounded
                    // so a pathological appender cannot spin forever.
                    if followup_continuations < Self::MAX_FOLLOWUP_CONTINUATIONS
                        && self.has_unanswered_user_message(&current_session).await
                    {
                        followup_continuations = followup_continuations.saturating_add(1);
                        if let Some(ref tracker) = self.stall_tracker {
                            tracker.record_activity().await;
                        }
                        iterations = iterations.saturating_add(1);
                        // Re-check the iteration cap here too: the follow-up
                        // continuation bumps `iterations` and loops, so without
                        // this a steering message arriving each final turn drives
                        // the run past `max_iterations` (and past hit_limit /
                        // HitMaxIterations accounting). Mirror the Continue arm.
                        if let Some(limit) = cap {
                            if iterations >= limit {
                                self.hit_limit.store(true, Ordering::Relaxed);
                                self.set_terminate_reason(TerminateReason::HitMaxIterations {
                                    used: iterations.try_into().unwrap_or(u32::MAX),
                                });
                                self.fire_boundary_grace_turn(
                                    &current_session,
                                    callback,
                                    iterations,
                                    crate::harness::agent::think::GraceReason::MaxIterations,
                                    cancel,
                                )
                                .await;
                                callback.on_complete();
                                break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                            }
                        }
                        continue;
                    }
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::Completed);
                }
            }
        };

        // P2: read the last AssistantMessage text once at exit so the
        // SessionCompleted trace event is self-sufficient. Trace consumers
        // no longer need to re-read the session log to recover `final_text`.
        // A read failure here is non-fatal — the trace event is still emitted
        // with `final_text: None`, matching the legacy behaviour.
        // Must happen before the `current_session` move below.
        let final_text = self
            .deps
            .session
            .get_events(&current_session, None, None)
            .await
            .ok()
            .and_then(|events| {
                events.iter().rev().find_map(|r| match &r.event {
                    // Stop at the genuine last assistant turn: returning `None`
                    // when both text and thinking are empty made find_map walk
                    // back and surface an older turn's text as this run's
                    // final_text. Always resolve at the first one encountered.
                    SessionEvent::AssistantMessage { content, .. } => {
                        Some(if content.text.is_empty() {
                            content.thinking.clone().unwrap_or_default()
                        } else {
                            content.text.clone()
                        })
                    }
                    _ => None,
                })
            });

        // Store the final session id — either the original or a split child.
        *self
            .final_session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(current_session.clone());

        let terminate_reason = Some(self.terminate_reason());
        let duration_ms = Some(self.duration_ms());
        let token_breakdown = Some(self.token_breakdown());
        let tool_timeline = self.tool_timeline();

        match result {
            Ok(outcome) => {
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: matches!(
                        outcome,
                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                    ),
                    final_text: final_text.clone(),
                    terminate_reason: terminate_reason.clone(),
                    duration_ms,
                    token_breakdown: token_breakdown.clone(),
                    tool_timeline: tool_timeline.clone(),
                });
                Ok(())
            }
            Err(e) => {
                let error_class = e.class();
                tracing::warn!(
                    ?current_session,
                    ?error_class,
                    error = %e,
                    "harness session ended in error",
                );
                #[allow(clippy::match_same_arms)]
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome: session_outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: false,
                    final_text,
                    terminate_reason,
                    duration_ms,
                    token_breakdown,
                    tool_timeline,
                });
                Err(e)
            }
        }
    }

    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let iterations = count_assistant_messages(&events).saturating_add(1);
        let tool_calls_made = count_tool_calls(&events);
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut history = std::collections::VecDeque::new();
        self.run_turn_internal(
            session_id,
            callback,
            iterations,
            tool_calls_made,
            &mut history,
            &cancel,
        )
        .await
        .map(|step| step.state)
    }
}

/// Index at which events "since the last `AssistantMessage`" begin.
pub(crate) fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map_or(0, |idx| idx + 1)
}

/// Count `AssistantMessage` events in the log.
pub(crate) fn count_assistant_messages(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count()
}

/// Count `ToolCallRequested` events.
pub(crate) fn count_tool_calls(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count()
}

/// Serialize each `NativeToolCall` as a JSON `tool_use` block.
///
/// Gemini 3's `thought_signature` is persisted alongside the call so it
/// survives the session-event round-trip and is replayed on later turns. The
/// key is omitted entirely when absent (other providers / older Gemini), so
/// pre-existing session logs are unaffected. Reader: `parse_tool_use_block`.
pub(crate) fn tool_use_blocks(tool_calls: &[NativeToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|c| {
            let mut block = json!({
                "type": "tool_use",
                "id": c.id,
                "name": c.name,
                "input": c.arguments,
            });
            if let Some(sig) = &c.thought_signature {
                block["thought_signature"] = Value::String(sig.clone());
            }
            block
        })
        .collect()
}

/// Stable serialization of a JSON value for memo cache keys.
pub(crate) fn canonical_json_string(value: &Value) -> String {
    fn canon(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::with_capacity(keys.len());
                for k in keys {
                    out.insert(k.clone(), canon(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&canon(value)).expect("canonical JSON serialization should never fail")
}

/// Find the most recent `TurnStarted` id; generate a fresh one if none exists.
pub(crate) fn current_turn_id(events: &[SessionEventRecord]) -> TurnId {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}

/// Sum the provider-reported token components for one LLM call.
///
/// `total_tokens` = `input + output + cache_read + cache_creation`. Cache
/// components default to 0 when the provider omits them. `thinking_tokens`
/// is intentionally excluded for a consistent cross-provider token
/// definition: Anthropic/OpenAI already fold thinking tokens into
/// `output_tokens` (counting them would double-count), and Gemini reports
/// `thoughtsTokenCount` separately. If Gemini thinking tokens ever need to
/// be counted, adjust the sum here.
fn turn_token_total(usage: &Option<crate::providers::adapter::TokenUsage>) -> u64 {
    match usage {
        None => 0,
        Some(u) => {
            u64::from(u.input_tokens)
                + u64::from(u.output_tokens)
                + u64::from(u.cache_read_tokens.unwrap_or(0))
                + u64::from(u.cache_creation_tokens.unwrap_or(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;
    use std::future::Future;
    use std::pin::Pin;

    use crate::error::Result as AlephResult;
    use crate::harness::callback::NoopHarnessCallback;
    use crate::harness::deps::HarnessDeps;
    use crate::harness::trait_def::Harness;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
    use crate::providers::AiProvider;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::ToolOutput;
    use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::{SessionId, SessionService};
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use serde_json::{json, Value};

    #[test]
    fn failure_streak_counts_majority_failure_not_just_total_failure() {
        // (executed, errors) -> should this turn increment the streak?
        assert!(super::is_failure_turn(0, 2)); // total failure
        assert!(super::is_failure_turn(1, 3)); // majority failure (1 ok, 3 err)
        assert!(!super::is_failure_turn(3, 1)); // mostly success → not a failure turn
        assert!(!super::is_failure_turn(2, 0)); // clean → not a failure turn
    }

    #[test]
    fn failure_streak_resets_only_on_clean_turn() {
        assert!(super::is_clean_turn(2, 0)); // zero errors → reset
        assert!(!super::is_clean_turn(2, 1)); // any error → hold/increment, don't reset
        assert!(!super::is_clean_turn(0, 1));
    }

    struct AlwaysOkTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for AlwaysOkTools {
        async fn execute(
            &self,
            _name: &str,
            _args: Value,
        ) -> Result<ToolOutput, crate::tools::service::ToolError> {
            Ok(ToolOutput {
                value: Value::String("ok".to_string()),
                metadata: Default::default(),
            })
        }

        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            vec![]
        }

        async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }

        fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
            Arc::from(vec![])
        }
    }

    struct LoopingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AiProvider for LoopingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ProviderResponse {
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: "loop".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({"text": "loop"}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    ..Default::default()
                })
            })
        }

        fn name(&self) -> &str {
            "looping"
        }

        fn color(&self) -> &str {
            "#ff0000"
        }
    }

    struct SleepingProvider {
        sleep: std::time::Duration,
    }

    impl AiProvider for SleepingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let sleep = self.sleep;
            Box::pin(async move {
                tokio::time::sleep(sleep).await;
                Ok(ProviderResponse::text_only("done".to_string()))
            })
        }

        fn name(&self) -> &str {
            "sleeping"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct UsageProvider {
        usage: crate::providers::adapter::TokenUsage,
    }

    impl AiProvider for UsageProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let usage = self.usage.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: Some("done".to_string()),
                    stop_reason: StopReason::EndTurn,
                    usage: Some(usage),
                    ..Default::default()
                })
            })
        }

        fn name(&self) -> &str {
            "usage"
        }

        fn color(&self) -> &str {
            "#00ff00"
        }
    }

    async fn fresh_session(agent_id: &str) -> (Arc<InProcessActorSessionService>, SessionId) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).expect("migrate");
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session = Arc::new(InProcessActorSessionService::new(store));
        let sid = SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        session
            .emit_event(
                &sid,
                SessionEvent::TurnStarted {
                    turn_id: uuid::Uuid::new_v4(),
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &sid,
                SessionEvent::UserMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text: "go".into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                    synthetic: false,
                },
            )
            .await
            .unwrap();
        (session, sid)
    }

    // ---- Pi `getFollowUpMessages` parity: has_unanswered_user_message ----

    /// Build a fresh, empty in-memory session (no seeded events, unlike
    /// [`fresh_session`]) so each follow-up test controls the exact ordering.
    fn empty_session(agent_id: &str) -> (Arc<InProcessActorSessionService>, SessionId) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).expect("migrate");
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session = Arc::new(InProcessActorSessionService::new(store));
        let sid = SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        (session, sid)
    }

    /// Minimal harness for unit-testing `has_unanswered_user_message`. The LLM
    /// provider is never invoked — only the session dep is exercised.
    fn followup_harness(session: Arc<InProcessActorSessionService>) -> super::AgentHarness {
        let deps = HarnessDeps {
            session,
            tools: Arc::new(AlwaysOkTools),
            sandbox: Arc::new(crate::sandbox::NoopSandbox),
            llm: Arc::new(SleepingProvider {
                sleep: std::time::Duration::from_secs(0),
            }),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            preflight_pipeline: None,
            trace_sink: None,
            system_prompt: None,
            system_prompt_parts: None,
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            turn_budget: None,
            result_store: None,
            session_epoch_registrar: None,
            tool_signal_sink: std::sync::Arc::new(
                crate::memory::tool_signal_sink::NoopToolSignalSink,
            ),
            in_flight_tool_calls: None,
            parallel_tool_concurrency: None,
        };
        super::AgentHarness::new(deps)
    }

    async fn emit_user(
        session: &InProcessActorSessionService,
        sid: &SessionId,
        text: &str,
        synthetic: bool,
    ) {
        session
            .emit_event(
                sid,
                SessionEvent::UserMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text: text.into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                    synthetic,
                },
            )
            .await
            .unwrap();
    }

    async fn emit_assistant(session: &InProcessActorSessionService, sid: &SessionId, text: &str) {
        session
            .emit_event(
                sid,
                SessionEvent::AssistantMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text: text.into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
    }

    /// Set the per-turn prompt-boundary watermark the way `run_turn_internal`
    /// would after reading a log of `len` events. Mirrors the production store so
    /// the follow-up tests exercise the real watermark logic.
    fn set_watermark(harness: &super::AgentHarness, len: usize) {
        harness
            .last_prompt_log_len
            .store(len, crate::sync_primitives::Ordering::Relaxed);
    }

    /// Normal completion: the model's last act is its assistant turn, so there
    /// is nothing to follow up on — the loop must be allowed to terminate.
    #[tokio::test]
    async fn no_followup_when_assistant_is_last() {
        let (session, sid) = empty_session("fu-normal");
        emit_user(&session, &sid, "do the thing", false).await;
        emit_assistant(&session, &sid, "done").await;
        let harness = followup_harness(session);
        // Final turn built its prompt from the single leading user message.
        set_watermark(&harness, 1);
        assert!(!harness.has_unanswered_user_message(&sid).await);
    }

    /// The boundary race: a real user message lands *after* the final assistant
    /// turn. The model has not seen it, so the loop must continue.
    #[tokio::test]
    async fn followup_when_user_message_arrives_after_final_turn() {
        let (session, sid) = empty_session("fu-race");
        emit_user(&session, &sid, "do the thing", false).await;
        emit_assistant(&session, &sid, "done").await;
        // Steering message injected after the closing turn committed.
        emit_user(&session, &sid, "actually, also do this", false).await;
        let harness = followup_harness(session);
        set_watermark(&harness, 1);
        assert!(harness.has_unanswered_user_message(&sid).await);
    }

    /// The harder boundary race the watermark exists for: a steering message is
    /// injected *while the final turn's LLM call is still streaming*, so it lands
    /// in the log *before* the assistant message that turn goes on to commit.
    /// A naive "is there a user after the last assistant" test misses it; the
    /// watermark (the turn's pre-prompt boundary) catches it.
    #[tokio::test]
    async fn followup_when_user_injected_during_final_turn() {
        let (session, sid) = empty_session("fu-during");
        emit_user(&session, &sid, "do the thing", false).await;
        // Final turn read the log here (len == 1) and started streaming.
        emit_user(&session, &sid, "wait, also handle errors", false).await;
        // ...then the turn commits its assistant message, now positioned *after*
        // the injected steering message.
        emit_assistant(&session, &sid, "done").await;
        let harness = followup_harness(session);
        set_watermark(&harness, 1);
        assert!(harness.has_unanswered_user_message(&sid).await);
    }

    /// A *synthetic* trailing user message (verifier-veto / grace-turn nudge) is
    /// harness-internal, not genuine user input — it must not trigger a
    /// follow-up continuation.
    #[tokio::test]
    async fn no_followup_for_synthetic_trailing_message() {
        let (session, sid) = empty_session("fu-synthetic");
        emit_user(&session, &sid, "do the thing", false).await;
        emit_assistant(&session, &sid, "done").await;
        emit_user(&session, &sid, "[verifier veto] keep going", true).await;
        let harness = followup_harness(session);
        set_watermark(&harness, 1);
        assert!(!harness.has_unanswered_user_message(&sid).await);
    }

    /// Before the first assistant turn there is no completed turn to "follow up"
    /// on; the leading user prompt must not be mistaken for a late arrival.
    #[tokio::test]
    async fn no_followup_before_first_assistant_turn() {
        let (session, sid) = empty_session("fu-pre");
        emit_user(&session, &sid, "do the thing", false).await;
        let harness = followup_harness(session);
        assert!(!harness.has_unanswered_user_message(&sid).await);
    }

    #[test]
    fn turn_token_total_sums_four_components() {
        use crate::providers::adapter::TokenUsage;
        let usage = Some(TokenUsage {
            input_tokens: 100,
            output_tokens: 250,
            cache_read_tokens: Some(40),
            cache_creation_tokens: Some(10),
            thinking_tokens: Some(999),
            cost: None,
        });
        // 100 + 250 + 40 + 10 = 400. thinking_tokens (999) is excluded.
        assert_eq!(super::turn_token_total(&usage), 400);
    }

    #[test]
    fn turn_token_total_none_usage_is_zero() {
        assert_eq!(super::turn_token_total(&None), 0);
    }

    #[test]
    fn turn_token_total_treats_missing_cache_as_zero() {
        use crate::providers::adapter::TokenUsage;
        let usage = Some(TokenUsage {
            input_tokens: 7,
            output_tokens: 11,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        });
        assert_eq!(super::turn_token_total(&usage), 18);
    }

    #[tokio::test]
    async fn harness_accumulates_provider_token_usage() {
        use crate::providers::adapter::TokenUsage;
        let provider: Arc<dyn AiProvider> = Arc::new(UsageProvider {
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: Some(5),
                cache_creation_tokens: Some(3),
                thinking_tokens: Some(99),
                cost: None,
            },
        });

        let (session, sid) = fresh_session("test-tokens").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            preflight_pipeline: None,
            trace_sink: None,
            system_prompt: None,
            system_prompt_parts: None,
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            turn_budget: None,
            result_store: None,
            session_epoch_registrar: None,
            tool_signal_sink: std::sync::Arc::new(
                crate::memory::tool_signal_sink::NoopToolSignalSink,
            ),
            in_flight_tool_calls: None,
            parallel_tool_concurrency: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

        // Single text-only turn: input + output + cache_read + cache_creation
        // = 10 + 20 + 5 + 3 = 38. thinking_tokens (99) is excluded.
        assert_eq!(harness.total_tokens(), 38);

        // P2: token_breakdown captures per-component figures including the
        // reasoning slot that `total_tokens` deliberately drops.
        let bd = harness.token_breakdown();
        assert_eq!(bd.input, 10);
        assert_eq!(bd.output, 20);
        assert_eq!(bd.cache_read, 5);
        assert_eq!(bd.cache_creation, 3);
        assert_eq!(bd.reasoning, 99);
        assert_eq!(
            bd.total(),
            38,
            "breakdown.total() must agree with total_tokens()"
        );

        // Clean text-only run keeps the default Completed terminate reason.
        assert_eq!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::Completed,
        );
        // Wall-clock duration was stamped on entry — non-zero (or zero if
        // the run finished within sub-ms; just check it doesn't panic).
        let _ = harness.duration_ms();
    }

    #[tokio::test]
    async fn max_iterations_stops_runaway_loop() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(LoopingProvider {
            calls: calls.clone(),
        });

        let (session, sid) = fresh_session("test-cap").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            preflight_pipeline: None,
            trace_sink: None,
            system_prompt: None,
            system_prompt_parts: None,
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            turn_budget: None,
            result_store: None,
            session_epoch_registrar: None,
            tool_signal_sink: std::sync::Arc::new(
                crate::memory::tool_signal_sink::NoopToolSignalSink,
            ),
            in_flight_tool_calls: None,
            parallel_tool_concurrency: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = harness.run(&sid, &mut cb, &cancel).await;
        assert!(result.is_ok(), "run should succeed even when capped");
        assert!(harness.hit_limit(), "hit_limit should be true after cap");
        // 3 capped iterations = 3 provider calls, plus 1 grace turn fired by
        // the max_iterations cap (C1) — LoopingProvider never produces
        // terminal text, so the grace turn fires once more.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);

        // P2: terminate_reason precisely identifies the max-iter cap (the
        // legacy hit_limit:bool could not distinguish this from stall /
        // verifier-veto / consecutive-failure / turn-timeout).
        assert_eq!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::HitMaxIterations { used: 3 },
        );

        // P2: tool_timeline captures one entry per Act-phase tool call. The
        // dedup memo (H4) is per-batch, so each of the 3 single-call turns
        // executes its own `echo` invocation.
        let timeline = harness.tool_timeline();
        assert_eq!(timeline.len(), 3, "one timeline entry per Act-phase call");
        assert!(timeline.iter().all(|i| i.success));
        assert_eq!(timeline[0].name, "echo");
    }

    #[tokio::test]
    async fn turn_timeout_returns_ok_with_hit_limit_not_err() {
        let provider: Arc<dyn AiProvider> = Arc::new(SleepingProvider {
            sleep: std::time::Duration::from_millis(200),
        });

        let (session, sid) = fresh_session("test-turn-timeout-ok-path").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            preflight_pipeline: None,
            trace_sink: None,
            system_prompt: None,
            system_prompt_parts: None,
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: Some(std::time::Duration::from_millis(20)),
            turn_budget: None,
            result_store: None,
            session_epoch_registrar: None,
            tool_signal_sink: std::sync::Arc::new(
                crate::memory::tool_signal_sink::NoopToolSignalSink,
            ),
            in_flight_tool_calls: None,
            parallel_tool_concurrency: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = harness.run(&sid, &mut cb, &cancel).await;
        assert!(result.is_ok(), "turn timeout should return Ok, not Err");
        assert!(
            harness.hit_limit(),
            "hit_limit should be true after timeout"
        );

        // P2: TurnTimeout is distinguishable from other cap reasons. The
        // phase string comes from `TurnPhase::Think` because the sleeping
        // provider hangs the LLM call (not a tool call).
        match harness.terminate_reason() {
            crate::orchestrator::dispatch::TerminateReason::TurnTimeout { phase, .. } => {
                assert!(
                    phase.to_lowercase().contains("think"),
                    "expected think-phase timeout, got phase={phase}",
                );
            }
            other => panic!("expected TurnTimeout, got {other:?}"),
        }
    }
}
