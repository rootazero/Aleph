//! Context Budget — pressure sensing and the compaction circuit breaker.
//!
//! This module replaces the old `ToolCompactorConfig` with a richer abstraction
//! that tracks context window pressure across turns and issues directives to the
//! agent loop (compact, split the session, or compact to fit).

pub mod cheap_passes;
pub mod preflight;
pub mod pressure;

use crate::context::budget::pressure::{
    estimate_message_tokens_split, estimate_tokens_aware, MessageTokenSplit,
};
use crate::providers::message::UnifiedMessage;
use crate::providers::reasoning_replay::ReasoningReplay;

// =============================================================================
// ContextPressure
// =============================================================================

/// Snapshot of context window utilization at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPressure {
    /// Estimated tokens currently consumed (overhead + messages).
    pub used_tokens: usize,
    /// Total token budget for the model.
    pub budget_tokens: usize,
    /// Ratio of used / budget (0.0 .. 1.0+).
    pub ratio: f64,
    /// Tokens consumed by system prompt + tool definitions (the "bootstrap" cost).
    pub overhead_tokens: usize,
    /// Tokens available for conversation messages (budget - overhead).
    pub available_for_messages: usize,
}

/// Threshold at which bootstrap overhead triggers a warning log.
const OVERHEAD_WARNING_RATIO: f64 = 0.30;
/// Threshold at which bootstrap overhead triggers a critical warning.
const OVERHEAD_CRITICAL_RATIO: f64 = 0.50;
/// Minimum pressure drop (fraction of budget) for a compaction to count as
/// "effective" and re-arm the circuit breaker. Below this the breaker keeps
/// its count, so a run of ineffective compactions still escalates to a session
/// split (and, once splits are exhausted, `CompactToFit`) — the anti-thrash
/// safety path borrowed from hermes, but never a hard stop.
const COMPACTION_EFFECTIVE_DROP: f64 = 0.05;
/// EWMA weight on the newest observation when smoothing the calibration factor.
/// Low enough to ride through transient noise (cache swings, recovery resends),
/// high enough to converge within a handful of turns.
const CALIBRATION_ALPHA: f64 = 0.3;
/// Clamp band for a single observation's correction factor. A char-ratio
/// estimate is rarely off by more than ~2× either way; values outside this band
/// signal noise (a mid-flight resend, a degenerate provider usage report) and
/// are clamped rather than allowed to whipsaw the budget. `pub(crate)` so the
/// prompt-side gate (`thinker::prompt_budget`) seeds from the same band.
pub(crate) const CALIBRATION_MIN: f64 = 0.25;
pub(crate) const CALIBRATION_MAX: f64 = 4.0;
/// Width of the "preventive band" sitting just below the warning threshold.
/// Inside `[warning - PREVENTIVE_BAND_WIDTH, warning)` the deterministic
/// preflight cheap passes are allowed to fire; below it they are a no-op.
///
/// This is headroom's "compress only when it pays" principle (its
/// `live_zone_only` / pressure-aware aggressiveness) mapped onto Aleph: the
/// lossy cheap passes (tool-result pruning, historical image stripping) shed
/// context the model may still want, so they should act only once the context
/// is genuinely filling up — not on every turn of a near-empty conversation.
/// 0.10 below the default 0.70 warning reproduces the historical 0.60 gate of
/// `FileOpSupersedeStage` exactly, so the default escalation ladder stays
/// byte-compatible: `< floor` keep everything → `[floor, warning)` cheap passes
/// → `≥ warning` side-channel LLM compaction.
const PREVENTIVE_BAND_WIDTH: f64 = 0.10;

impl ContextPressure {
    /// Compute a pressure snapshot.
    ///
    /// `tool_schema_tokens` is the caller-precomputed token cost of the tool
    /// schema actually sent to the provider. Keeping it a plain `usize` (rather
    /// than a `&[ToolDefinition]`) decouples this module from any tool-def type
    /// and lets the harness count the exact wire schema (`tool_metadata::ToolDefinition`).
    ///
    /// `messages` is any sequence of message borrows — in production the
    /// reasoning-replay projection, borrowed (`ReasoningReplay::projected`), so
    /// computing pressure never clones the history.
    pub(crate) fn compute<I>(
        messages: I,
        system_prompt: &str,
        tool_schema_tokens: usize,
        token_budget: u64,
        ratio: f64,
    ) -> Self
    where
        I: IntoIterator,
        I::Item: std::borrow::Borrow<UnifiedMessage>,
    {
        Self::compute_with_split(
            messages,
            system_prompt,
            tool_schema_tokens,
            token_budget,
            ratio,
        )
        .0
    }

    /// [`Self::compute`], also returning the messages' estimate split by kind
    /// — from the same single pass, so the split sums to the snapshot's
    /// message tokens exactly.
    pub(crate) fn compute_with_split<I>(
        messages: I,
        system_prompt: &str,
        tool_schema_tokens: usize,
        token_budget: u64,
        ratio: f64,
    ) -> (Self, MessageTokenSplit)
    where
        I: IntoIterator,
        I::Item: std::borrow::Borrow<UnifiedMessage>,
    {
        // Content-aware estimation: `ratio` is the prose anchor, but CJK/code
        // content overrides it with denser ratios. This fixes the flat-ratio
        // blind spot (a fixed 3.5 under-counts CJK ~2.3× and code ~1.4×) that
        // made the budget sensor overflow on the first turn of a CJK/code-heavy
        // conversation — before the EWMA calibration in `observe_actual_usage`
        // ever sees a provider response. The calibration then refines this more
        // accurate base, converging faster than off a flat estimate.
        let prompt_tokens = estimate_tokens_aware(system_prompt, ratio);
        let overhead = prompt_tokens + tool_schema_tokens;
        // Per-message, content-aware AND image-aware: `text_content()` drops image
        // blocks, so summing only the text estimate counts screenshots as zero
        // tokens. `estimate_message_tokens_aware` adds the per-image charge so a
        // vision-heavy context reports its true pressure and compaction fires in
        // time. Image-free messages are byte-identical to the old text estimate.
        let mut split = MessageTokenSplit::default();
        for m in messages {
            split += estimate_message_tokens_split(std::borrow::Borrow::borrow(&m), ratio);
        }
        let used = overhead + split.total();
        let budget: usize = token_budget.try_into().unwrap_or(usize::MAX);
        let pressure = Self {
            used_tokens: used,
            budget_tokens: budget,
            ratio: if budget == 0 {
                1.0
            } else {
                used as f64 / budget as f64
            },
            overhead_tokens: overhead,
            available_for_messages: budget.saturating_sub(overhead),
        };
        (pressure, split)
    }

    /// Scale every token figure by a calibration `factor` (observed / estimated)
    /// and recompute the ratio, keeping the snapshot self-consistent.
    ///
    /// `factor == 1.0` (the uncalibrated default) returns `self` unchanged, so
    /// the budget is byte-identical until the first real provider observation
    /// arrives via [`ContextBudget::observe_actual_usage`].
    fn calibrated(self, factor: f64) -> Self {
        if (factor - 1.0).abs() < f64::EPSILON {
            return self;
        }
        let scale = |v: usize| ((v as f64) * factor).round() as usize;
        let used = scale(self.used_tokens);
        let overhead = scale(self.overhead_tokens);
        Self {
            used_tokens: used,
            budget_tokens: self.budget_tokens,
            ratio: if self.budget_tokens == 0 {
                1.0
            } else {
                used as f64 / self.budget_tokens as f64
            },
            overhead_tokens: overhead,
            available_for_messages: self.budget_tokens.saturating_sub(overhead),
        }
    }
}

// =============================================================================
// LoopDirective
// =============================================================================

/// Directive issued by the context budget to the agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDirective {
    /// Context is within budget — proceed normally.
    Continue,
    /// Context exceeds warning threshold — compact tool results before the next LLM call.
    CompactAndContinue,
    /// Context is critically full — compact aggressively until it fits
    /// (LLM summary → deterministic truncation floor) and CONTINUE. Replaces
    /// the old `FinalReply` hard-stop on the pressure path so a run can never
    /// terminate merely because the context filled up. See
    /// `context::compact::fit::compact_to_fit`.
    CompactToFit,
    /// In-place compaction is not keeping pressure down — split the session:
    /// continue the run in a fresh child session (epoch + 1) seeded with a
    /// summary + fresh tail. See `context::compact::session_split`.
    SplitSession,
}

// =============================================================================
// ContextBudgetConfig
// =============================================================================

/// Configuration for constructing a `ContextBudget`.
#[derive(Debug, Clone)]
pub struct ContextBudgetConfig {
    /// Total token budget for the model context window.
    pub token_budget: u64,
    /// Fraction of budget at which compaction triggers (e.g. 0.70).
    pub warning_threshold: f64,
    /// Fraction of budget at which pressure is critical and the deterministic
    /// compact-to-fit floor engages (e.g. 0.85).
    pub critical_threshold: f64,
    /// Characters-per-token ratio for estimation.
    pub token_estimate_ratio: f64,
    /// Number of recent messages to leave untouched during compaction.
    pub fresh_tail_count: usize,
    /// Token budget for a single summarizer-input call, derived from the
    /// summarizer model's own context window (see
    /// `deps_builder::context_budget::derive_summarizer_input_budget`):
    /// `min(SUMMARIZER_INPUT_TOKEN_BUDGET, window / 4)` — never above the
    /// historical 48k constant, smaller when the (possibly flash-tier)
    /// summarizer's window could not hold 48k of transcript plus the prompt
    /// scaffold and the summary output. A fixed 48k against a 64k-window
    /// cheap model overflowed the summarizer itself, so every compaction
    /// silently degraded to first-line truncation.
    pub summarizer_input_budget: usize,
    /// Max consecutive compaction attempts before circuit breaker trips.
    pub circuit_breaker_max: usize,
    /// Max session-splits allowed in one run before a circuit-breaker trip
    /// falls back to `CompactToFit`. Default 3.
    pub max_splits: usize,
}

impl ContextBudgetConfig {
    /// Fill-ratio floor below which the deterministic preflight cheap passes
    /// are a no-op — the bottom of the "preventive band" (see
    /// [`PREVENTIVE_BAND_WIDTH`]). Derived from the *configured* (and possibly
    /// per-model-overridden) [`warning_threshold`](Self::warning_threshold) so
    /// the cheap-pass gate tracks the same threshold that governs LLM
    /// compaction, instead of a hardcoded magic ratio that could drift above a
    /// custom warning line. Clamped at `0.0`, so a tiny warning threshold
    /// simply keeps the cheap passes always-on (their previous behaviour).
    #[must_use]
    pub(crate) fn preventive_floor(&self) -> f64 {
        (self.warning_threshold - PREVENTIVE_BAND_WIDTH).max(0.0)
    }
}

// =============================================================================
// CompactionCircuitBreaker
// =============================================================================

/// Tracks consecutive compaction attempts. If compaction keeps firing without
/// the pressure dropping, we escalate (`SplitSession`, then `CompactToFit` once
/// splits are exhausted) instead of looping forever.
#[derive(Debug)]
struct CompactionCircuitBreaker {
    max_consecutive: usize,
    consecutive_count: usize,
}

impl CompactionCircuitBreaker {
    const fn new(max: usize) -> Self {
        Self {
            max_consecutive: max,
            consecutive_count: 0,
        }
    }

    /// Record that compaction was triggered. Returns true if the breaker has tripped.
    const fn record_compaction(&mut self) -> bool {
        self.consecutive_count += 1;
        self.consecutive_count >= self.max_consecutive
    }

    /// Reset the counter (called when pressure drops below warning, or after
    /// a compaction that actually reduced pressure).
    const fn reset(&mut self) {
        self.consecutive_count = 0;
    }
}

// =============================================================================
// ContextBudget
// =============================================================================

/// Orchestrator that combines pressure sensing and circuit breaking to issue
/// directives to the agent loop.
#[derive(Debug)]
pub struct ContextBudget {
    token_budget: u64,
    warning_threshold: f64,
    critical_threshold: f64,
    token_estimate_ratio: f64,
    fresh_tail_count: usize,
    circuit_breaker: CompactionCircuitBreaker,
    /// Last computed pressure snapshot, saved by `before_turn()`.
    last_pressure: Option<ContextPressure>,
    /// Number of session splits that have already occurred in this run.
    split_count: usize,
    /// Maximum session splits allowed before the circuit-breaker trip falls back to `CompactToFit`.
    max_splits: usize,
    /// Self-learning multiplier applied to the heuristic token estimate,
    /// calibrated against the provider's reported prompt size after each turn.
    /// `None` until the first observation — the estimate then runs uncalibrated
    /// (factor 1.0), keeping behaviour byte-identical to the pre-calibration path.
    calibration: Option<f64>,
    /// The primary target's reasoning policy: pressure counts the reasoning
    /// the wire will carry, via the same projection `HttpProvider` applies.
    reasoning_replay: ReasoningReplay,
    /// The run's provider clears old tool results server-side, so the prompt
    /// it reports is post-clearing while the estimate is not — that ratio is
    /// not a tokenizer ratio, and calibration must not learn it.
    server_clears_tool_results: bool,
    /// The session and turn this budget measures prompts for, when its runner
    /// told it ([`Self::publish_message_tokens_as`]). Each measured prompt's
    /// message split is then recorded on that turn's prompt-size record.
    breakdown_turn: Option<(String, u64)>,
}

impl ContextBudget {
    /// Create a new context budget from configuration.
    ///
    /// **Pre-condition (audit 2026-08-26):** `config.token_budget` MUST be
    /// `> 0`. A zero budget produces a permanent `CompactToFit` loop in
    /// `before_turn` (ratio is 1.0, every turn compacts, ratio stays at 1.0)
    /// because the compaction post-condition (`ratio < critical`) cannot be
    /// reached when the budget itself is zero. The production builder at
    /// `orchestrator::deps_builder::context_budget` already rejects
    /// `token_budget == 0` and returns `None`, so a live `ContextBudget`
    /// cannot be built with a zero budget in production. This assertion is
    /// the tripwire that keeps the invariant from drifting in tests or any
    /// future call site that constructs `ContextBudget` directly.
    ///
    /// `assert!` (not `debug_assert!`) so a misuse fails fast in release
    /// builds too — the failure mode is silent correctness loss, not a
    /// spurious runtime cost, so panicking is the right behavior.
    #[must_use]
    pub fn new(config: &ContextBudgetConfig) -> Self {
        assert!(
            config.token_budget > 0,
            "ContextBudgetConfig.token_budget must be > 0 (got 0); a zero budget \
             produces a permanent CompactToFit loop because the post-condition \
             `ratio < critical_threshold` is unreachable. Construct via \
             `orchestrator::deps_builder::context_budget` which validates this."
        );
        Self {
            token_budget: config.token_budget,
            warning_threshold: config.warning_threshold,
            critical_threshold: config.critical_threshold,
            token_estimate_ratio: config.token_estimate_ratio,
            fresh_tail_count: config.fresh_tail_count,
            circuit_breaker: CompactionCircuitBreaker::new(config.circuit_breaker_max),
            last_pressure: None,
            split_count: 0,
            max_splits: config.max_splits,
            calibration: None,
            reasoning_replay: ReasoningReplay::default(),
            server_clears_tool_results: false,
            breakdown_turn: None,
        }
    }

    /// Count reasoning as `replay` sends it (the serving provider's
    /// [`AiProvider::reasoning_replay`](crate::providers::AiProvider::reasoning_replay)).
    pub fn set_reasoning_replay(&mut self, replay: ReasoningReplay) {
        self.reasoning_replay = replay;
    }

    /// The reasoning policy pressure is counted under.
    #[must_use]
    pub const fn reasoning_replay(&self) -> &ReasoningReplay {
        &self.reasoning_replay
    }

    /// The run's provider clears old tool results server-side
    /// ([`AiProvider::clears_tool_results_server_side`](crate::providers::AiProvider::clears_tool_results_server_side)).
    /// Freezes calibration for this budget: see [`Self::observe_actual_usage`].
    pub fn set_server_clears_tool_results(&mut self, clears: bool) {
        self.server_clears_tool_results = clears;
    }

    /// Record every measured prompt's message tokens, split by kind, on
    /// `session_key`'s prompt-size record for `context.breakdown`. The figures
    /// are the uncalibrated estimate, like every other breakdown row.
    ///
    /// Call it after the run's `record_turn`: the budget binds to the turn the
    /// record holds now, so a later run's record never receives this run's
    /// figures. With no record (or no registry) there is nothing to bind to,
    /// and nothing is recorded.
    pub fn publish_message_tokens_as(&mut self, session_key: impl Into<String>) {
        let session_key = session_key.into();
        self.breakdown_turn = crate::thinker::prompt_size_registry::global_prompt_size_registry()
            .and_then(|reg| reg.current_stamp(&session_key))
            .map(|turn| (session_key, turn));
    }

    fn publish(&self, split: MessageTokenSplit) {
        let Some((key, turn)) = self.breakdown_turn.as_ref() else {
            return;
        };
        if let Some(reg) = crate::thinker::prompt_size_registry::global_prompt_size_registry() {
            reg.record_messages(key, *turn, split);
        }
    }

    /// Total token budget.
    #[must_use]
    pub const fn token_budget(&self) -> u64 {
        self.token_budget
    }

    /// Characters-per-token ratio.
    #[must_use]
    pub const fn token_estimate_ratio(&self) -> f64 {
        self.token_estimate_ratio
    }

    /// Fraction of budget at which context is considered critically full.
    #[must_use]
    pub const fn critical_threshold(&self) -> f64 {
        self.critical_threshold
    }

    /// Fresh tail count for compaction.
    #[must_use]
    pub const fn fresh_tail_count(&self) -> usize {
        self.fresh_tail_count
    }

    /// Last computed pressure snapshot from `before_turn()`.
    #[must_use]
    pub const fn last_pressure(&self) -> Option<&ContextPressure> {
        self.last_pressure.as_ref()
    }

    /// Compute a read-only context pressure snapshot **without** mutating any
    /// internal state (calibration, circuit breaker, `last_pressure`).
    ///
    /// [`ContextBudget::before_turn`] is the stateful path that issues
    /// directives and records the snapshot; `peek_pressure` is for callers that
    /// need the *current* fill ratio purely to gate a decision — e.g. the
    /// preflight pipeline deciding whether pressure-sensitive cheap passes such
    /// as `FileOpSupersedeStage` should fire. It applies the same content-aware
    /// estimate and calibration factor as `before_turn`, so the ratio the
    /// preflight gate sees matches the ratio the budget check computes moments
    /// later on the (preflight-trimmed) message list.
    #[must_use]
    pub fn peek_pressure(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_schema_tokens: usize,
    ) -> ContextPressure {
        ContextPressure::compute(
            self.reasoning_replay.projected(messages),
            system_prompt,
            tool_schema_tokens,
            self.token_budget,
            self.token_estimate_ratio,
        )
        .calibrated(self.calibration.unwrap_or(1.0))
    }

    /// Evaluate context pressure before a turn and return a directive.
    ///
    /// `tool_schema_tokens` is the token cost of the tool schema sent to the
    /// provider — see [`ContextPressure::compute`].
    pub fn before_turn(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_schema_tokens: usize,
    ) -> LoopDirective {
        let (pressure, split) = ContextPressure::compute_with_split(
            self.reasoning_replay.projected(messages),
            system_prompt,
            tool_schema_tokens,
            self.token_budget,
            self.token_estimate_ratio,
        );
        let pressure = pressure.calibrated(self.calibration.unwrap_or(1.0));
        self.last_pressure = Some(pressure);
        self.publish(split);

        // Bootstrap overhead warnings (system prompt + tool definitions)
        if pressure.budget_tokens > 0 {
            let overhead_ratio = pressure.overhead_tokens as f64 / pressure.budget_tokens as f64;
            if overhead_ratio >= OVERHEAD_CRITICAL_RATIO {
                tracing::warn!(
                    target: "context_budget",
                    overhead = pressure.overhead_tokens,
                    budget = pressure.budget_tokens,
                    overhead_pct = format!("{:.0}%", overhead_ratio * 100.0),
                    "Bootstrap overhead consuming >50% of context budget — consider reducing tools or system prompt"
                );
            } else if overhead_ratio >= OVERHEAD_WARNING_RATIO {
                tracing::info!(
                    target: "context_budget",
                    overhead = pressure.overhead_tokens,
                    budget = pressure.budget_tokens,
                    overhead_pct = format!("{:.0}%", overhead_ratio * 100.0),
                    "Bootstrap overhead consuming >30% of context budget"
                );
            }
        }

        if pressure.ratio >= self.critical_threshold {
            // Critical — compact aggressively until it fits, then continue.
            // Never a hard stop: a run cannot end just because context filled.
            tracing::warn!(
                target: "context_budget",
                used = pressure.used_tokens,
                budget = pressure.budget_tokens,
                ratio = pressure.ratio,
                "Critical context pressure — compacting to fit"
            );
            return LoopDirective::CompactToFit;
        }

        if pressure.ratio >= self.warning_threshold {
            // Warning — compact, but check circuit breaker
            if self.circuit_breaker.record_compaction() {
                if self.split_count < self.max_splits {
                    tracing::warn!(
                        target: "context_budget",
                        split_count = self.split_count,
                        "Compaction circuit breaker tripped — requesting session split"
                    );
                    return LoopDirective::SplitSession;
                }
                tracing::warn!(
                    target: "context_budget",
                    split_count = self.split_count,
                    "Compaction circuit breaker tripped and split cap reached — compacting to fit"
                );
                return LoopDirective::CompactToFit;
            }
            tracing::info!(
                target: "context_budget",
                used = pressure.used_tokens,
                budget = pressure.budget_tokens,
                ratio = pressure.ratio,
                "Warning context pressure — requesting compaction"
            );
            return LoopDirective::CompactAndContinue;
        }

        // Under threshold — reset circuit breaker
        self.circuit_breaker.reset();
        LoopDirective::Continue
    }

    /// Record the effect of a just-completed compaction on context pressure.
    ///
    /// Re-computes pressure on the post-compaction message list and compares it
    /// to the snapshot saved by [`ContextBudget::before_turn`]. If pressure
    /// dropped by at least [`COMPACTION_EFFECTIVE_DROP`] of the budget the
    /// compaction worked — reset the circuit breaker so it re-arms. Otherwise
    /// the breaker keeps its count, so a run of ineffective compactions still
    /// escalates to a session split (and, once splits are exhausted,
    /// `CompactToFit`).
    ///
    /// Without this call the breaker only ever increments: it would trip after
    /// `circuit_breaker_max` compaction turns even when compaction is healthy,
    /// terminating long tasks prematurely.
    pub fn note_compaction_effect(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_schema_tokens: usize,
    ) {
        let Some(before) = self.last_pressure else {
            return;
        };
        let (after, split) = ContextPressure::compute_with_split(
            self.reasoning_replay.projected(messages),
            system_prompt,
            tool_schema_tokens,
            self.token_budget,
            self.token_estimate_ratio,
        );
        let after = after.calibrated(self.calibration.unwrap_or(1.0));
        self.publish(split);
        if before.ratio - after.ratio >= COMPACTION_EFFECTIVE_DROP {
            self.circuit_breaker.reset();
        }
        self.last_pressure = Some(after);
    }

    /// Calibrate the heuristic token estimator against the provider's reported
    /// prompt size for the request that was just sent.
    ///
    /// `observed_prompt_tokens` is the ground-truth token count of the prompt
    /// (system + tools + messages) from [`crate::providers::adapter::TokenUsage::prompt_tokens_total`].
    /// The snapshot saved by [`ContextBudget::before_turn`] (or refreshed by
    /// [`ContextBudget::note_compaction_effect`]) is the calibrated estimate of
    /// that *same* prompt, so `observed / estimated` is the residual error of the
    /// current calibration. Backing the previous factor out yields the absolute
    /// correction, which is clamped (rejecting transient noise) and EWMA-smoothed
    /// into the running multiplier.
    ///
    /// The effect: the budget converges to *this conversation's* true tokenizer
    /// ratio — adapting to content mix, the provider's tokenizer, and cache
    /// behaviour that the fixed char-per-token ratio cannot capture. This is
    /// strictly an accuracy improvement to the estimate that already drives every
    /// compaction decision; it adds no new decision category and makes no LLM call.
    ///
    /// Not while the run's provider clears old tool results server-side: the
    /// reported prompt is then post-clearing and the estimate is not, so the
    /// ratio would sink the factor — and the runner carries the factor over to
    /// the next run on the same model, which may not clear (another host, OAuth,
    /// a failover, the setting off) and would then under-count and compact late.
    /// Such a run keeps whatever factor it was seeded with; its local estimate
    /// over-counts once the server starts clearing (a documented gap).
    pub fn observe_actual_usage(&mut self, observed_prompt_tokens: usize) {
        if self.server_clears_tool_results {
            return;
        }
        let Some(p) = self.last_pressure else {
            return;
        };
        if observed_prompt_tokens == 0 || p.used_tokens == 0 {
            return;
        }
        let prev = self.calibration.unwrap_or(1.0);
        // `p.used_tokens` already had `prev` applied, so multiply it back out to
        // recover the absolute observed/raw-estimate factor before smoothing.
        let absolute = (observed_prompt_tokens as f64 / p.used_tokens as f64) * prev;
        let factor = absolute.clamp(CALIBRATION_MIN, CALIBRATION_MAX);
        self.calibration = Some(match self.calibration {
            Some(prev) => CALIBRATION_ALPHA * factor + (1.0 - CALIBRATION_ALPHA) * prev,
            None => factor,
        });
    }

    /// Current calibration multiplier, if any observation has been recorded.
    /// Exposed for diagnostics/tests; `None` means the estimate is uncalibrated.
    #[must_use]
    pub const fn calibration(&self) -> Option<f64> {
        self.calibration
    }

    /// Seed the calibration multiplier before the first turn of a run.
    ///
    /// A fresh per-run budget starts uncalibrated, so the FIRST `before_turn`
    /// — the one carrying the full accumulated history, where heuristic drift
    /// is largest — always ran on the raw estimate. Callers that retain the
    /// factor a previous run converged to (keyed by model id: a factor learned
    /// under one tokenizer must never apply to another) inject it here right
    /// after [`ContextBudget::new`]. The seed is clamped to the same band as
    /// live observations, non-finite values are ignored, and
    /// [`ContextBudget::observe_actual_usage`] keeps refining it exactly as a
    /// mid-run factor. Breaker / split state is untouched — only
    /// estimator accuracy carries over.
    pub fn seed_calibration(&mut self, factor: f64) {
        if !factor.is_finite() {
            return;
        }
        self.calibration = Some(factor.clamp(CALIBRATION_MIN, CALIBRATION_MAX));
    }

    /// Record that a session-split completed. Increments the per-run split
    /// counter; once it reaches `max_splits`, further breaker trips fall back
    /// to `CompactToFit`.
    ///
    /// The run now continues in a child session, so the prompts this budget
    /// measures from here on are the child's: it stops recording them on the
    /// session it was told, and clears what it recorded there, rather than
    /// report the child's conversation as the parent's.
    pub fn record_split(&mut self) {
        self.split_count = self.split_count.saturating_add(1);
        self.circuit_breaker.reset();
        if let Some((key, stamp)) = self.breakdown_turn.take() {
            if let Some(reg) = crate::thinker::prompt_size_registry::global_prompt_size_registry() {
                reg.clear_messages(&key, stamp);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ContextBudgetConfig {
        ContextBudgetConfig {
            token_budget: 10_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            summarizer_input_budget: 48_000,
            circuit_breaker_max: 3,
            max_splits: 3,
        }
    }

    /// T-G8: a budget told its session records each measured prompt's
    /// message split on that session's turn — of the projection the target is
    /// sent, so a target that gets no reasoning shows none — and an untold
    /// budget records nothing.
    ///
    /// Mutation-checked: dropping `publish` from `before_turn`, or measuring
    /// `messages.iter()` instead of the projection, turns this red.
    #[test]
    fn a_measured_prompt_publishes_its_projected_split_for_its_session() {
        use crate::providers::message::ContentBlock;
        let registry = crate::thinker::prompt_size_registry::install_test_prompt_size_registry();
        let key = format!("test:breakdown:{}", uuid::Uuid::new_v4());
        let messages_of = |key: &str| registry.latest(key).and_then(|r| r.messages);
        let msgs = vec![
            UnifiedMessage::user("question"),
            UnifiedMessage::Assistant {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "reason about it ".repeat(50),
                        signature: Some("sig".into()),
                        earlier_turn: false,
                    },
                    ContentBlock::ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({ "command": "ls" }),
                        thought_signature: None,
                    },
                ],
            },
            UnifiedMessage::tool_result_json(
                "c1",
                "bash",
                serde_json::json!("some output line\n".repeat(100)),
                false,
            ),
        ];
        let mut budget = ContextBudget::new(&default_config());
        budget.publish_message_tokens_as(key.clone());
        let _ = budget.before_turn(&msgs, "", 0);
        registry.record_turn(&key, None, vec![]);
        assert_eq!(
            messages_of(&key),
            None,
            "a budget told before the turn was recorded has no turn to write to"
        );

        let mut budget = ContextBudget::new(&default_config());
        let _ = budget.before_turn(&msgs, "", 0);
        assert_eq!(messages_of(&key), None, "untold budgets publish nothing");

        budget.publish_message_tokens_as(key.clone());
        let _ = budget.before_turn(&msgs, "", 0);
        let kept = messages_of(&key).expect("published");
        assert!(
            kept.reasoning > 0 && kept.tool_results > 0 && kept.other > 0,
            "{kept:?}"
        );

        budget.set_reasoning_replay(ReasoningReplay::Drop);
        let _ = budget.before_turn(&msgs, "", 0);
        let dropped = messages_of(&key).expect("published");
        assert_eq!(
            dropped.reasoning, 0,
            "a target sent no reasoning shows none"
        );
        assert_eq!(dropped.tool_results, kept.tool_results);
    }

    /// F5: after a session split the run's prompts are the child's. The
    /// parent's record must read as unknown, never as the child's
    /// conversation — neither what was recorded before the split nor anything
    /// measured after it.
    ///
    /// Mutation-checked: leaving the binding in place in `record_split` turns
    /// this red.
    #[test]
    fn after_a_split_the_parent_record_holds_no_messages() {
        let registry = crate::thinker::prompt_size_registry::install_test_prompt_size_registry();
        let key = format!("test:split:{}", uuid::Uuid::new_v4());
        registry.record_turn(&key, None, vec![]);
        let msgs = vec![UnifiedMessage::user("question")];
        let mut budget = ContextBudget::new(&default_config());
        budget.publish_message_tokens_as(key.clone());
        let _ = budget.before_turn(&msgs, "", 0);
        assert!(registry.latest(&key).unwrap().messages.is_some());

        budget.record_split();
        assert_eq!(
            registry.latest(&key).unwrap().messages,
            None,
            "cleared at the split"
        );
        let _ = budget.before_turn(&msgs, "", 0);
        assert_eq!(
            registry.latest(&key).unwrap().messages,
            None,
            "the child's prompt is not written onto the parent"
        );
    }

    #[test]
    fn preventive_floor_is_warning_minus_band() {
        // Default warning 0.70 → floor 0.60, byte-compatible with the historical
        // hardcoded FileOpSupersedeStage gate.
        let cfg = default_config();
        assert!((cfg.preventive_floor() - 0.60).abs() < 1e-9);
        // Per-model override of the warning line moves the cheap-pass band with
        // it — the gate is no longer a magic constant.
        let high = ContextBudgetConfig {
            warning_threshold: 0.85,
            ..default_config()
        };
        assert!((high.preventive_floor() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn preventive_floor_clamps_at_zero() {
        // A tiny warning threshold must not produce a negative floor — the
        // cheap passes simply stay always-on (their pre-band behaviour).
        let cfg = ContextBudgetConfig {
            warning_threshold: 0.05,
            ..default_config()
        };
        assert!((cfg.preventive_floor() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_context_pressure_compute() {
        let msgs = vec![UnifiedMessage::user("Hello world")];
        let pressure = ContextPressure::compute(&msgs, "system", 0, 1000, 3.5);
        assert!(pressure.ratio < 1.0);
        assert!(pressure.used_tokens > 0);
        assert_eq!(pressure.budget_tokens, 1000);
        // Overhead should include system prompt tokens
        assert!(pressure.overhead_tokens > 0);
        assert!(pressure.available_for_messages < 1000);
        assert_eq!(
            pressure.available_for_messages,
            1000 - pressure.overhead_tokens
        );
    }

    #[test]
    fn test_context_pressure_cjk_not_undercounted() {
        // A CJK-heavy message must register far more pressure than a flat 3.5
        // ratio would report — the regression that let CJK sessions overflow the
        // provider context before compaction triggered. With CJK ratio 1.5 the
        // sensor should see ~2.3× the tokens a flat 3.5 estimate gave.
        let cjk = "这是一段很长的中文对话内容用来测试上下文预算传感器是否会低估中文消息的token数量从而导致在压缩触发之前就超出模型上下文窗口".repeat(3);
        let msgs = vec![UnifiedMessage::user(cjk.clone())];
        let aware = ContextPressure::compute(&msgs, "", 0, 100_000, 3.5);
        let chars = cjk.chars().count();
        let flat_used = (chars as f64 / 3.5).ceil() as usize;
        assert!(
            aware.used_tokens > flat_used,
            "content-aware sensor ({}) must exceed flat-3.5 ({flat_used}) for CJK",
            aware.used_tokens
        );
    }

    #[test]
    fn test_context_pressure_counts_images() {
        use crate::context::budget::pressure::IMAGE_TOKENS_ESTIMATE;
        use crate::providers::message::ContentBlock;

        // An image-bearing turn must register more pressure than the same text
        // alone: `text_content()` drops image blocks, so before this fix a
        // multi-megabyte screenshot counted as zero tokens and a vision session
        // under-reported pressure by ~IMAGE_TOKENS_ESTIMATE per image.
        let text_only = vec![UnifiedMessage::user("look at this")];
        let with_image = vec![UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "fake_base64".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Text {
                text: "look at this".to_string(),
                cache_control: None,
            },
        ])];

        let p_text = ContextPressure::compute(&text_only, "", 0, 100_000, 3.5);
        let p_image = ContextPressure::compute(&with_image, "", 0, 100_000, 3.5);
        assert_eq!(
            p_image.used_tokens,
            p_text.used_tokens + IMAGE_TOKENS_ESTIMATE,
            "an image turn must cost the text estimate plus one image charge"
        );
    }

    #[test]
    fn test_context_pressure_overhead_with_tools() {
        let msgs = vec![UnifiedMessage::user("hi")];
        // Non-zero tool-schema overhead must raise the overhead and the used total.
        let pressure = ContextPressure::compute(&msgs, "system", 400, 10_000, 3.5);
        let pressure_no_tools = ContextPressure::compute(&msgs, "system", 0, 10_000, 3.5);
        assert_eq!(
            pressure.overhead_tokens,
            pressure_no_tools.overhead_tokens + 400,
            "tool schema tokens must be added to overhead"
        );
        assert!(
            pressure.used_tokens > pressure_no_tools.used_tokens,
            "tools should increase used tokens"
        );
        assert!(
            pressure.available_for_messages < pressure_no_tools.available_for_messages,
            "tools should shrink the room left for messages"
        );
    }

    #[test]
    fn test_loop_directive_continue_under_threshold() {
        let config = default_config();
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("short")];
        let directive = budget.before_turn(&msgs, "sys", 0);
        assert_eq!(directive, LoopDirective::Continue);
    }

    #[test]
    fn test_circuit_breaker_trips() {
        let mut cb = CompactionCircuitBreaker::new(3);
        assert!(!cb.record_compaction());
        assert!(!cb.record_compaction());
        assert!(cb.record_compaction()); // 3rd time trips
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_compaction();
        cb.record_compaction();
        cb.reset();
        assert!(!cb.record_compaction()); // reset, so starts from 1
    }

    // --- before_turn pressure path tests ---

    #[test]
    fn test_before_turn_warning_returns_compact() {
        // token_estimate_ratio=1.0 → 1 char = 1 token, budget=1000
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 750 chars = 75% usage → Warning zone
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        let directive = budget.before_turn(&msgs, "", 0);
        assert_eq!(directive, LoopDirective::CompactAndContinue);
    }

    #[test]
    fn test_before_turn_critical_returns_compact_to_fit() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 900 chars = 90% usage → Critical zone
        let msgs = vec![UnifiedMessage::user("x".repeat(900))];
        let directive = budget.before_turn(&msgs, "", 0);
        assert_eq!(directive, LoopDirective::CompactToFit);
    }

    #[test]
    fn test_before_turn_circuit_breaker_escalates_to_split_session() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            circuit_breaker_max: 2,
            max_splits: 3,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 750 chars = Warning zone, stays in warning for 2+ turns → breaker trips
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        let d1 = budget.before_turn(&msgs, "", 0);
        assert_eq!(d1, LoopDirective::CompactAndContinue);
        let d2 = budget.before_turn(&msgs, "", 0);
        assert_eq!(d2, LoopDirective::SplitSession); // breaker tripped on 2nd attempt
    }

    #[test]
    fn note_compaction_effect_resets_breaker_when_effective() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            circuit_breaker_max: 2,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let big = vec![UnifiedMessage::user("x".repeat(750))];
        // Turn 1: warning → compact (breaker count = 1).
        assert_eq!(
            budget.before_turn(&big, "", 0),
            LoopDirective::CompactAndContinue
        );
        // Effective compaction: pressure drops far below the warning line.
        let small = vec![UnifiedMessage::user("x".repeat(100))];
        budget.note_compaction_effect(&small, "", 0);
        // Turn 2: still big → warning. The breaker was reset, so even with
        // max=2 it does NOT escalate to SplitSession — long tasks survive.
        assert_eq!(
            budget.before_turn(&big, "", 0),
            LoopDirective::CompactAndContinue
        );
    }

    #[test]
    fn note_compaction_effect_keeps_counting_when_ineffective() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            circuit_breaker_max: 2,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let big = vec![UnifiedMessage::user("x".repeat(750))];
        // Turn 1: warning → compact (breaker count = 1).
        assert_eq!(
            budget.before_turn(&big, "", 0),
            LoopDirective::CompactAndContinue
        );
        // Ineffective compaction: pressure barely moves → breaker not reset.
        budget.note_compaction_effect(&big, "", 0);
        // Turn 2: warning again → breaker count = 2 → trips. Under the split cap
        // the trip escalates to SplitSession (vs CompactAndContinue when reset).
        assert_eq!(budget.before_turn(&big, "", 0), LoopDirective::SplitSession);
    }

    #[test]
    fn test_new_rejects_zero_budget_at_construction() {
        // Audit 2026-08-26 F1: a zero budget used to silently produce a
        // permanent CompactToFit loop. The pre-condition assertion on
        // `ContextBudget::new` closes the gap; this test pins the tripwire
        // so the invariant cannot drift back via a future "let's relax
        // this" refactor. Use the inner `should_panic` (not the legacy
        // #[should_panic(expected = ...)] attribute) so the message is
        // printed verbatim if the assert is changed.
        let config = ContextBudgetConfig {
            token_budget: 0,
            ..default_config()
        };
        let result = std::panic::catch_unwind(|| {
            let _budget = ContextBudget::new(&config);
        });
        assert!(
            result.is_err(),
            "zero token_budget must fail at construction"
        );
    }

    #[test]
    fn circuit_breaker_trip_emits_split_session_when_under_cap() {
        let mut cfg = default_config();
        cfg.token_budget = 1000;
        cfg.warning_threshold = 0.70;
        cfg.critical_threshold = 0.85;
        cfg.token_estimate_ratio = 1.0;
        cfg.circuit_breaker_max = 2;
        cfg.max_splits = 3;
        let mut budget = ContextBudget::new(&cfg);
        // 750 chars = 75% usage → Warning zone (token_estimate_ratio=1.0)
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        let d1 = budget.before_turn(&msgs, "", 0);
        assert_eq!(d1, LoopDirective::CompactAndContinue);
        // 2nd call trips the breaker (circuit_breaker_max=2); split_count=0 < max_splits=3
        let directive = budget.before_turn(&msgs, "", 0);
        assert_eq!(
            directive,
            LoopDirective::SplitSession,
            "first breaker trip under the split cap must request a session split",
        );
    }

    #[test]
    fn split_session_falls_back_to_compact_to_fit_at_cap() {
        let mut cfg = default_config();
        cfg.token_budget = 1000;
        cfg.warning_threshold = 0.70;
        cfg.critical_threshold = 0.85;
        cfg.token_estimate_ratio = 1.0;
        cfg.circuit_breaker_max = 2;
        cfg.max_splits = 1;
        let mut budget = ContextBudget::new(&cfg);
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        // Prime the breaker: first call → CompactAndContinue
        budget.before_turn(&msgs, "", 0);
        // Trip the breaker: split_count=0 < max_splits=1 → SplitSession
        let first = budget.before_turn(&msgs, "", 0);
        assert_eq!(first, LoopDirective::SplitSession);
        budget.record_split(); // split_count → 1 == max_splits
                               // Re-prime: reset the consecutive counter so we can trip again cleanly
                               // (pressure is still in warning band; breaker consecutive_count is >= max after the trip)
                               // We need circuit_breaker_max more warning-band calls to trip again.
        budget.before_turn(&msgs, "", 0); // consecutive_count = 1 (CompactAndContinue)
                                          // Trip the breaker again: split_count=1 == max_splits=1 → CompactToFit
        let second = budget.before_turn(&msgs, "", 0);
        assert_eq!(
            second,
            LoopDirective::CompactToFit,
            "once max_splits is reached, the breaker trip falls back to CompactToFit",
        );
    }

    #[test]
    fn critical_pressure_requests_compact_to_fit_not_final_reply() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // Build a message list whose estimated tokens blow past 0.85 * 1000.
        let big = vec![UnifiedMessage::user("x".repeat(8000))]; // ~2285 tokens @3.5 → ratio > 2.0
        let directive = budget.before_turn(&big, "", 0);
        assert_eq!(
            directive,
            LoopDirective::CompactToFit,
            "critical pressure must compact-to-fit, never hard-stop with FinalReply"
        );
    }

    // --- calibration (server-observed token feedback) tests ---

    #[test]
    fn observe_actual_usage_noop_without_last_pressure() {
        let mut budget = ContextBudget::new(&default_config());
        budget.observe_actual_usage(5000);
        assert_eq!(budget.calibration(), None, "no snapshot → no calibration");
    }

    #[test]
    fn observe_actual_usage_ignores_zero_observation() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("x".repeat(100))];
        budget.before_turn(&msgs, "", 0);
        budget.observe_actual_usage(0);
        assert_eq!(budget.calibration(), None);
    }

    /// A run whose provider clears old tool results server-side reports a
    /// post-clearing prompt against a pre-clearing estimate. It must learn
    /// nothing from that: the factor it was seeded with — the one the runner
    /// writes back to the cross-run carry-over — is left exactly as it was,
    /// and an unseeded one stays unseeded (so nothing is written back at all).
    /// The positive half: the same observation on a non-clearing budget moves
    /// the factor.
    #[test]
    fn a_clearing_run_neither_learns_nor_moves_the_carried_factor() {
        let config = ContextBudgetConfig {
            token_budget: 10_000,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let msgs = vec![UnifiedMessage::user("x".repeat(1000))];
        let run = |clears: bool, seed: Option<f64>| {
            let mut budget = ContextBudget::new(&config);
            budget.set_server_clears_tool_results(clears);
            if let Some(seed) = seed {
                budget.seed_calibration(seed);
            }
            budget.before_turn(&msgs, "", 0);
            // The server cleared most of it: it reports far fewer tokens.
            budget.observe_actual_usage(300);
            budget.calibration()
        };

        assert_eq!(run(true, Some(1.2)), Some(1.2));
        assert_eq!(run(true, None), None);
        let learned = run(false, Some(1.2)).expect("a non-clearing run learns");
        assert!(
            learned < 1.2,
            "precondition: the observation moves the factor"
        );
    }

    #[test]
    fn observe_actual_usage_first_factor_is_ratio() {
        // ratio=1.0 → 100 chars + 0 overhead = 100 estimated tokens.
        let config = ContextBudgetConfig {
            token_budget: 10_000,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("x".repeat(100))];
        budget.before_turn(&msgs, "", 0);
        // Provider says the prompt was actually 150 tokens → estimate was 1.5× low.
        budget.observe_actual_usage(150);
        let cal = budget.calibration().expect("calibration set");
        assert!(
            (cal - 1.5).abs() < 1e-6,
            "first factor == raw ratio, got {cal}"
        );
    }

    #[test]
    fn observe_actual_usage_clamps_outliers() {
        let config = ContextBudgetConfig {
            token_budget: 10_000,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("x".repeat(100))];
        budget.before_turn(&msgs, "", 0);
        // Absurd 100× observation (e.g. a degenerate usage report) must clamp.
        budget.observe_actual_usage(10_000);
        let cal = budget.calibration().expect("calibration set");
        assert!(
            (cal - CALIBRATION_MAX).abs() < 1e-6,
            "outlier clamped to {CALIBRATION_MAX}, got {cal}"
        );
    }

    #[test]
    fn observe_actual_usage_converges_and_is_stable_at_truth() {
        let config = ContextBudgetConfig {
            token_budget: 100_000,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("x".repeat(1000))];
        // Estimate is consistently 1000; truth is consistently 1300 (estimate
        // 30% low). Repeated observation should converge the multiplier to ~1.3.
        for _ in 0..20 {
            budget.before_turn(&msgs, "", 0);
            budget.observe_actual_usage(1300);
        }
        let cal = budget.calibration().expect("calibration set");
        assert!(
            (cal - 1.3).abs() < 0.05,
            "calibration should converge to ~1.3, got {cal}"
        );
        // Once converged, the calibrated estimate matches truth, so the absolute
        // factor recovered each turn stays ~1.3 (no drift toward 1.0).
        budget.before_turn(&msgs, "", 0);
        let calibrated_used = budget.last_pressure().unwrap().used_tokens;
        assert!(
            (calibrated_used as i64 - 1300).abs() < 80,
            "calibrated estimate should track truth (~1300), got {calibrated_used}"
        );
    }

    #[test]
    fn calibration_makes_underestimate_trigger_compaction() {
        // budget=1000, ratio=1.0. Raw estimate of a 600-char message = 600 tokens
        // = 60% → below the 70% warning line → Continue. But the provider reports
        // the prompt is really 800 tokens (80%). After calibration the budget
        // should see the true pressure and request compaction.
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.95,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("x".repeat(600))];
        // Turn 1: uncalibrated → 60% → Continue.
        assert_eq!(budget.before_turn(&msgs, "", 0), LoopDirective::Continue);
        // Provider ground-truth: the prompt was actually 800 tokens.
        budget.observe_actual_usage(800);
        // Turn 2: calibrated estimate ≈ 800 → 80% ≥ warning → compaction.
        assert_eq!(
            budget.before_turn(&msgs, "", 0),
            LoopDirective::CompactAndContinue,
            "calibration should surface true pressure the heuristic missed"
        );
    }
}
