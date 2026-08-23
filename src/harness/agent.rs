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
//!   * `guardrails.rs` — `apply_tool_call_guardrail`
//!   * `prompt.rs` — `build_prompt` (sync per-turn message assembler)

use crate::sync_primitives::{AtomicBool, AtomicU32, AtomicU64, Mutex, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::harness::callback::HarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{HarnessError, TurnState, TurnStep};
use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};
use crate::providers::adapter::NativeToolCall;

use crate::session::events::{SessionEvent, SessionEventRecord, TurnId};
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

/// A "failure turn" for the consecutive-failure watchdog: the turn made no net
/// progress because failures outnumber successes. Pure structural count — no
/// judgment about whether a *successful* call was actually useful (R10-safe).
pub(crate) const fn is_failure_turn(executed: usize, errors: usize) -> bool {
    errors > executed
}

/// A "clean turn" resets the streak: zero tool errors this turn. An interleaved
/// single success no longer zeroes a churning failure streak — only a fully
/// clean turn does.
pub(crate) const fn is_clean_turn(_executed: usize, errors: usize) -> bool {
    errors == 0
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
    /// Historically set when `context_budget.before_turn` returned
    /// `FinalReply`; that path now escalates through `SplitSession` →
    /// `CompactToFit`'s deterministic truncation floor instead (never a hard
    /// stop). Surfaced through [`AgentHarness::hit_limit`] so the
    /// orchestrator bridge can populate `FlowOutcome::hit_limit`.
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
    /// Most recent LLM call's raw usage (last-writer-wins snapshot), refreshed
    /// in [`AgentHarness::accumulate_token_breakdown`] alongside the cumulative
    /// [`token_breakdown`]. Unlike the cumulative counter this is *replaced*
    /// each call, so it reflects current context-window occupancy (last prompt
    /// sent + that call's output) rather than the run total — the cumulative
    /// input would blow past the window and peg the gauge at 100%. Read after
    /// the run via [`AgentHarness::last_turn_context_tokens`].
    last_turn_usage: Mutex<Option<crate::providers::adapter::TokenUsage>>,
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
    /// the LLM call, at most `MAX_REACTIVE_COMPACT_ATTEMPTS` times per run
    /// (defined in `context::compact::rescue` — currently 2). The atomic is
    /// bumped *before* the rescue
    /// attempt so concurrent paths cannot race past the cap. Repeated
    /// overflows after
    /// summarisation indicate fundamentally oversized input rather than a
    /// recoverable burst. R10-safe: pure round scheduling, no policy choice.
    pub(super) reactive_compact_attempts: AtomicU32,
    /// Seq of the last persisted event covered by the most recent turn's
    /// prompt (captured in `think.rs` right after `get_events`, before that
    /// turn appended anything). `0` is the cold sentinel — the store assigns
    /// seqs from 1, so 0 means "no prompt built yet this run" and both
    /// consumers treat it as "nothing to compare against". The follow-up
    /// boundary check ([`AgentHarness::has_unanswered_user_message`]) treats
    /// any non-synthetic `UserMessage` with seq > watermark as input the model
    /// never saw; the consecutive-failure watchdog uses the same boundary to
    /// fetch only the just-finished turn's events instead of the full log.
    ///
    /// Why a watermark and not "user after last assistant": a steering message
    /// injected *while the final turn's LLM call is still streaming* lands in
    /// the log *before* the assistant message that turn commits, so the
    /// positional "is there a user after the last assistant" test silently
    /// misses it. Late arrivals sit at seq > watermark regardless of the
    /// assistant's position. Seq-based (not an index) so consumers can ask the
    /// store for the tail directly (`get_events(from: watermark+1)`) rather
    /// than re-fetching the whole log every call. Mechanical (R10-safe): one
    /// store per turn, no decision. The loop runs one turn at a time, so
    /// `Relaxed` ordering is sufficient.
    pub(super) last_prompt_seq: AtomicU64,
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
            last_turn_usage: Mutex::new(None),
            started_at: OnceLock::new(),
            final_session_id: Mutex::new(None),
            recent_failures: Mutex::new(std::collections::HashSet::new()),
            reactive_compact_attempts: AtomicU32::new(0),
            last_prompt_seq: AtomicU64::new(0),
        }
    }

    /// Atomically reserve a reactive-compaction rescue slot; `false` once the
    /// run's budget is spent, and the caller must then fall back to the
    /// deterministic floor. The slot is per-run state and lives here; the cap
    /// is *policy* and lives with the algorithm, in
    /// [`crate::context::compact::rescue::MAX_REACTIVE_COMPACT_ATTEMPTS`] —
    /// this reads it rather than hardcoding the same number, because a cap the
    /// slot ignores is a lie: raising the const would silently change nothing.
    /// Compare-and-swap, so two concurrent paths can never both rescue.
    pub(super) fn try_reserve_reactive_compact(&self) -> bool {
        self.reactive_compact_attempts
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |spent| {
                (spent < crate::context::compact::rescue::MAX_REACTIVE_COMPACT_ATTEMPTS)
                    .then_some(spent + 1)
            })
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

    /// Tokens occupying the model's context window as of the most recent LLM
    /// call: the last prompt actually sent plus that call's output. `0` when no
    /// LLM round-trip happened this run. Gauge numerator — distinct from the
    /// run-cumulative [`AgentHarness::total_tokens`].
    pub fn last_turn_context_tokens(&self) -> u32 {
        let occupancy = self
            .last_turn_usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map_or(
                0,
                crate::providers::adapter::TokenUsage::context_occupancy_tokens,
            );
        u32::try_from(occupancy).unwrap_or(u32::MAX)
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
            // Snapshot the latest call's usage (last-writer-wins) so the
            // orchestrator can report *current* context-window occupancy, not
            // the run-cumulative input. Kept in lockstep with the cumulative
            // fold above — both update from the same `usage` in one place.
            *self
                .last_turn_usage
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(u.clone());
        }
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
    /// Returns `true` iff a non-synthetic [`SessionEvent::UserMessage`] landed
    /// *after* the last turn's prompt boundary (`last_prompt_seq`): that turn
    /// built its prompt without the message, so the model has not yet seen,
    /// let alone answered, that input. The mid-loop steering path
    /// ([`crate::gateway::execution_engine`]) documents exactly this race: an
    /// injection that lands during the closing LLM call would otherwise sit at
    /// the tail of the log until the user sent a *second* message. Continuing
    /// the loop here answers it in the same run.
    ///
    /// When the newest user message *precedes* the boundary, the model already
    /// saw it and chose to stop; that decision is the model's (R7), so we
    /// return `false` and let the loop terminate. The cold sentinel (0 = no
    /// prompt built yet this run) also returns `false` — before the first
    /// prompt the leading user message is the turn being processed, not a late
    /// arrival. A read error likewise returns `false` — failing closed keeps a
    /// transient session-store glitch from spinning the loop. Only the tail
    /// past the watermark is fetched (seq-ranged `get_events`), so the check
    /// stays O(new events) per call instead of re-reading the whole log.
    /// Purely positional: no intent, completion, or relevance judgement is
    /// made (R10-safe scaffolding).
    pub(crate) async fn has_unanswered_user_message(&self, session: &SessionId) -> bool {
        let watermark = self.last_prompt_seq.load(Ordering::Relaxed);
        if watermark == 0 {
            return false;
        }
        let tail = match self
            .deps
            .session
            .get_events(session, Some(watermark.saturating_add(1)), None)
            .await
        {
            Ok(e) => e,
            Err(_) => return false,
        };
        tail.iter().any(|r| {
            matches!(
                &r.event,
                SessionEvent::UserMessage { synthetic, .. } if !*synthetic
            )
        })
    }
}

impl AgentHarness {
    /// Loop `run_turn_internal` until `Done` or a cap trips. `cancel` is checked
    /// before every turn; a cancelled token aborts with
    /// [`HarnessError::Cancelled`] — the orchestrator distinguishes cooperative
    /// abort from natural completion. The terminal callback is
    /// `on_complete_with_outcome`, fired by `AgentHarnessRunner` once the full
    /// `FlowOutcome` exists.
    pub async fn run(
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
                    if tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await
                    .is_err()
                    {
                        tracing::warn!(
                            ?current_session,
                            grace_budget_secs = GRACE_TIMEOUT_BUDGET.as_secs(),
                            "grace turn exceeded its budget and was abandoned; \
                             terminal summary may be incomplete",
                        );
                    }
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
                Err(HarnessError::StalledTurn { elapsed }) => {
                    tracing::warn!(
                        ?current_session,
                        ?elapsed,
                        "per-turn timeout tripped; forcing Done with hit_limit",
                    );
                    self.hit_limit.store(true, Ordering::Relaxed);
                    self.set_terminate_reason(TerminateReason::TurnTimeout {
                        phase: "Think".into(),
                        elapsed_ms: elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    });
                    if tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await
                    .is_err()
                    {
                        tracing::warn!(
                            ?current_session,
                            grace_budget_secs = GRACE_TIMEOUT_BUDGET.as_secs(),
                            "grace turn exceeded its budget and was abandoned; \
                             terminal summary may be incomplete",
                        );
                    }
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                }
                Err(e) => {
                    // Only a genuine cancel records `Cancelled`. The blanket
                    // overwrite that used to stand here erased causes the loop
                    // had already recorded — most visibly
                    // `ReactiveCompactExhausted`, whose sole producer
                    // (`RescueHost::mark_rescue_exhausted`) sets it and then
                    // returns the very error that lands in this arm. That
                    // variant exists so an exhausted compact-and-retry stops
                    // leaking out as an anonymous `HarnessError::Llm`; setting
                    // it and overwriting it three frames later undid the wiring
                    // its own doc points at, and every other error path was
                    // being told to call itself a user cancellation.
                    if matches!(e, HarnessError::Cancelled) {
                        self.set_terminate_reason(TerminateReason::Cancelled);
                    }
                    break Err(e);
                }
                Ok(TurnStep {
                    state: TurnState::Continue,
                    executed,
                    vetoed,
                    split_child,
                }) => {
                    // A split turn's tail lives in the CHILD session, so the
                    // parent-watermark fetch in the watchdog below would read
                    // an empty/mismatched tail as a clean turn and silently
                    // reset the consecutive-failure streak. Skip the watchdog
                    // entirely on split turns.
                    let split = split_child.is_some();
                    if let Some(child) = split_child {
                        current_session = child;
                        // Reset the prompt watermark when switching sessions:
                        // `last_prompt_seq` was captured from the PARENT's
                        // event log, but the child's seqs are independent.
                        // Using a parent watermark on the child's log makes
                        // `has_unanswered_user_message` fetch the wrong tail
                        // (or an empty one when the parent's seq exceeds the
                        // child's), silently suppressing follow-up detection
                        // on the child until a second user message arrives
                        // (H3 in review/harness-statics). Resetting to 0 is
                        // the safe default — the next Think on the child
                        // re-captures a proper watermark, and the follow-up
                        // check returns false until then.
                        self.last_prompt_seq.store(0, Ordering::Relaxed);
                    }
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    // Consecutive-failure watchdog (structural). Count this
                    // turn's tool errors vs successful executions over the
                    // events since the last assistant message.
                    if !vetoed && !split {
                        // Fetch only the just-finished turn's tail (everything
                        // appended past its prompt boundary: the assistant
                        // message plus tool results), not the full log — keeps
                        // the per-turn cost proportional to the turn instead of
                        // O(session length) every iteration.
                        let prompt_seq = self.last_prompt_seq.load(Ordering::Relaxed);
                        let events = self
                            .deps
                            .session
                            .get_events(&current_session, Some(prompt_seq.saturating_add(1)), None)
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
                                    let warn = SessionEvent::synthetic_user(
                                        uuid::Uuid::new_v4(),
                                        crate::thinker::nudges::SOFT_FAILURE_WARNING.to_string(),
                                    );
                                    if let Err(e) =
                                        self.deps.session.emit_event(&current_session, warn).await
                                    {
                                        tracing::warn!(
                                            ?current_session,
                                            ?e,
                                            "soft-failure warning emit failed"
                                        );
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
                    // arrived past the final turn's prompt boundary
                    // (`last_prompt_seq`), the model has not seen it —
                    // continue the loop so it is answered in this same run
                    // instead of waiting for the user to send a second message.
                    // The watermark catches messages injected *during* the final
                    // LLM call too, which land *before* the just-committed
                    // assistant message and so escape a naive "user after last
                    // assistant" test. Purely positional (R10-safe) and bounded
                    // so a pathological appender cannot spin forever.
                    //
                    // `!self.hit_limit()` is what separates "the model chose to
                    // stop" from "the harness was forced to stop". A `Done`
                    // carrying `hit_limit` comes from the verifier `Halt` path
                    // in `run_turn_internal`, whose `TerminateReason::StopHookHalt`
                    // is documented as "the loop ends immediately … a permanent
                    // stop signal from policy". Without this clause a steering
                    // message that happened to land during the halted turn
                    // resumed the loop anyway — the policy gate held or not
                    // depending purely on message timing, and the terminate
                    // reason kept saying `StopHookHalt` while further turns ran.
                    // Cheap atomic first so the common path never pays the
                    // seq-ranged store read.
                    if !self.hit_limit()
                        && followup_continuations < Self::MAX_FOLLOWUP_CONTINUATIONS
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
                                break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                            }
                        }
                        continue;
                    }
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
                // A harness error is not a cancellation. Only
                // `HarnessError::Cancelled` — the cooperative abort token — is;
                // a provider auth failure, a session-store write error, an
                // output-guardrail block and an exhausted reactive compaction
                // are failures. The previous shape computed `error_class` and
                // then fanned all four of its variants into `Cancelled`, so
                // every trace surface rendered a dead run as
                // `AgentTracePresentationStatus::Info` labelled "cancelled" —
                // softer than a `HitLimit` cap, and indistinguishable from the
                // user pressing stop. The session log has always told the two
                // apart (`RunOutcome::Errored` vs `Cancelled`, set in
                // `harness_bridge::runner_impl` right before this event is
                // emitted); this makes the trace projection agree with the log
                // instead of contradicting it. `error_class` stays a log field:
                // all four of its classes mean "this run failed" to a trace
                // reader, and a match that fans four arms into one value is a
                // classification that was never made.
                let session_outcome = if matches!(e, HarnessError::Cancelled) {
                    crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                } else {
                    crate::harness::trace::LoopTraceSessionOutcome::Failed
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
}

/// Index at which events "since the last `AssistantMessage`" begin.
pub(crate) fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map_or(0, |idx| idx + 1)
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
    // serde_json::to_string on a serde_json::Value is infallible for all
    // values produced by the harness (non-finite floats are the only failure
    // mode and do not occur in tool args/results). The empty fallback is
    // defensive and is never exercised in practice.
    serde_json::to_string(&canon(value)).unwrap_or_default()
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
pub(crate) fn turn_token_total(usage: &Option<crate::providers::adapter::TokenUsage>) -> u64 {
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
