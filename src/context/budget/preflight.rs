//! `PreflightPipeline` — ordered execution of the deterministic "cheap pass"
//! stages (see `cheap_passes/`) over the message list before the more
//! expensive LLM compaction is considered.
//!
//! Every stage is a zero-LLM structural transform (tool-result pruning,
//! file-op supersession, historical image stripping) gated by the preventive
//! band: below the configured fill-ratio floor
//! ([`PreflightPipeline::with_min_pressure_ratio`]) the whole pipeline is a
//! no-op. The trait is `async` only because the pipeline sits on the agent's
//! async path — implementations are deterministic and make no LLM calls.

use super::ContextPressure;
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;

/// How far the pruning boundary may slide, in messages, before the pipeline is
/// willing to rewrite history again.
///
/// Every cheap-pass rewrite lands *below* all three message-level cache
/// breakpoints (they sit on the last few messages, the boundary is at
/// `len - fresh_tail`), so any rewrite re-bills the entire message prefix at
/// `cache_creation` (1.25x) instead of reading it at `cache_read` (~0.1x). With
/// the boundary sliding forward one notch per message, that happened *every
/// turn*: one newly-unprotected tool result was rewritten, and the whole
/// prefix went with it.
///
/// Quantising the boundary makes it step instead of slide. Inside a step the
/// candidate set is fixed, so the rendered history is byte-identical turn after
/// turn and the prefix survives; the 1.25x re-creation is paid once per step
/// rather than once per turn. A tool loop adds roughly two messages a turn, so
/// 16 amortises it over ~8 turns.
const DEFAULT_CUT_QUANTUM: usize = 16;

/// Fraction of the model's token budget a rewrite must free before it is worth
/// invalidating the message prefix.
///
/// The tokens a cheap pass removes were going to be billed at the *cached* rate
/// anyway, so the saving is ~0.1x per turn while the invalidation costs
/// ~1.15x of the whole prefix once — on cost alone these passes rarely pay
/// back. What they actually buy is headroom against the far more expensive LLM
/// compaction, and a rewrite that frees a rounding error buys none of it.
const DEFAULT_MIN_SAVINGS_RATIO: f64 = 0.02;

/// Translate a quantised cut point back into the tail count stages consume.
///
/// Stages prune `[0, len - fresh_tail)`. Rounding that cut *down* to a multiple
/// of `quantum` holds it still for `quantum` messages at a time. `quantum <= 1`
/// is the historical sliding behaviour.
fn quantized_tail(len: usize, fresh_tail_count: usize, quantum: usize) -> usize {
    if quantum <= 1 {
        return fresh_tail_count;
    }
    let cut = len.saturating_sub(fresh_tail_count);
    len.saturating_sub((cut / quantum) * quantum)
}

// =============================================================================
// PreflightStage trait
// =============================================================================

/// A single async pre-flight stage that can free tokens from the message list
/// before the main synchronous compaction pipeline runs.
#[async_trait]
pub trait PreflightStage: Send + Sync {
    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Attempt to free tokens from the message list.
    ///
    /// `fresh_tail_count` messages at the tail are protected and must not be
    /// modified. Returns the number of tokens freed (estimated).
    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize;
}

// =============================================================================
// PreflightPipeline
// =============================================================================

/// Executes an ordered list of async `PreflightStage`s, accumulating the total
/// tokens freed across all stages.
pub struct PreflightPipeline {
    stages: Vec<Box<dyn PreflightStage>>,
    /// Fill-ratio floor below which the whole pipeline is a no-op — the bottom
    /// of the "preventive band". `0.0` (the [`new`](Self::new) default) keeps
    /// the historical always-run behaviour; production wires this from
    /// [`ContextBudgetConfig::preventive_floor`](super::ContextBudgetConfig::preventive_floor)
    /// so the lossy cheap passes only act once the context is genuinely filling
    /// up. Centralising the gate here gives all stages one consistent
    /// aggressiveness knob (headroom's pressure-aware compression), rather than
    /// each stage carrying its own ad-hoc threshold.
    min_pressure_ratio: f64,
    /// Messages the pruning boundary must advance before history is rewritten
    /// again. See [`DEFAULT_CUT_QUANTUM`]. `1` (the [`new`](Self::new) default)
    /// is the historical per-message slide.
    cut_quantum: usize,
    /// Fraction of the token budget a rewrite must free to be committed. See
    /// [`DEFAULT_MIN_SAVINGS_RATIO`]. `0.0` (the default) commits anything.
    min_savings_ratio: f64,
    /// Fill ratio at/above which both guards are waived, because the LLM
    /// compactor is about to rewrite history anyway — the prefix is lost either
    /// way, so take every token. Production wires this from
    /// [`ContextBudgetConfig::warning_threshold`](super::ContextBudgetConfig::warning_threshold).
    /// `1.0` (the default) also keeps the degenerate no-budget path — which
    /// hands the pipeline a `ratio: 1.0` placeholder — on its historical
    /// always-run behaviour.
    flush_ratio: f64,
}

impl PreflightPipeline {
    /// Create a new pipeline with the given ordered stages. The preventive-band
    /// floor defaults to `0.0` (always run); set it with
    /// [`with_min_pressure_ratio`](Self::with_min_pressure_ratio).
    #[must_use]
    pub fn new(stages: Vec<Box<dyn PreflightStage>>) -> Self {
        Self {
            stages,
            min_pressure_ratio: 0.0,
            cut_quantum: 1,
            min_savings_ratio: 0.0,
            flush_ratio: 1.0,
        }
    }

    /// Create an empty pipeline (no-op).
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Set the cache-stability guards: how far the pruning boundary must move
    /// before history is rewritten again, how much a rewrite must free to be
    /// worth the invalidation, and the fill ratio at/above which both are
    /// waived. See the field docs and [`run`](Self::run).
    #[must_use]
    pub fn with_cache_stability(
        mut self,
        cut_quantum: usize,
        min_savings_ratio: f64,
        flush_ratio: f64,
    ) -> Self {
        self.cut_quantum = cut_quantum;
        self.min_savings_ratio = min_savings_ratio;
        self.flush_ratio = flush_ratio;
        self
    }

    /// Set the preventive-band floor: below this fill ratio
    /// [`run`](Self::run) skips every stage and frees nothing.
    #[must_use]
    pub fn with_min_pressure_ratio(mut self, ratio: f64) -> Self {
        self.min_pressure_ratio = ratio;
        self
    }

    /// Run every stage in order over `messages`, returning the tokens freed.
    async fn run_stages(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        let mut total_freed: usize = 0;
        for stage in &self.stages {
            let freed = stage.prepare(messages, pressure, fresh_tail_count).await;
            total_freed += freed;

            tracing::info!(
                target: "preflight_pipeline",
                stage = stage.name(),
                tokens_freed = freed,
                total_freed,
                "Preflight stage completed"
            );
        }
        total_freed
    }

    /// Run all stages in order, returning the total tokens freed.
    ///
    /// Two guards protect the prompt-cache prefix, both waived at/above
    /// [`flush_ratio`](Self::flush_ratio) where compaction is about to break it
    /// anyway:
    ///
    /// 1. The pruning boundary is **quantised** so it steps rather than slides
    ///    (see [`DEFAULT_CUT_QUANTUM`]). Within a step the candidate set — and
    ///    therefore the rendered history — is identical turn after turn.
    /// 2. The whole pass is **staged and only committed** when it frees more
    ///    than [`min_savings_ratio`](Self::min_savings_ratio) of the budget.
    ///    A discarded pass leaves `messages` untouched, so its candidates stay
    ///    pending and are re-offered — with everything that became eligible
    ///    since — on the next turn. That accumulation is what makes batching
    ///    work without any per-session bookkeeping: the message list *is* the
    ///    pending set.
    ///
    /// Both totals are monotone across steps (a later step's candidate set is a
    /// superset), so the commit decision never oscillates: once a pass commits,
    /// every later one does too.
    pub async fn run(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        // Preventive-band gate: with plenty of context headroom, keep history
        // verbatim. The lossy cheap passes (tool-result pruning, historical
        // image stripping) permanently shed context the model may still want,
        // so paying that cost on a near-empty conversation is pure loss — the
        // budget sensor still escalates to LLM compaction at the warning line.
        if pressure.ratio < self.min_pressure_ratio {
            tracing::debug!(
                target: "preflight_pipeline",
                ratio = pressure.ratio,
                floor = self.min_pressure_ratio,
                "context below preventive band — preflight cheap passes skipped"
            );
            return 0;
        }

        // The compactor is about to rewrite history: the prefix is lost either
        // way, so drop both cache guards and free every token available. Also
        // the only path that mutates in place — no staging clone on the hot,
        // largest-context path.
        if pressure.ratio >= self.flush_ratio {
            return self.run_stages(messages, pressure, fresh_tail_count).await;
        }

        let effective_tail = quantized_tail(messages.len(), fresh_tail_count, self.cut_quantum);
        let mut staged = messages.clone();
        let total_freed = self.run_stages(&mut staged, pressure, effective_tail).await;

        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation
        )]
        let floor = (pressure.budget_tokens as f64 * self.min_savings_ratio) as usize;
        if total_freed < floor {
            tracing::debug!(
                target: "preflight_pipeline",
                tokens_freed = total_freed,
                floor,
                ratio = pressure.ratio,
                "preflight rewrite discarded — it would re-bill the whole message \
                 prefix for less than it frees; candidates stay pending"
            );
            return 0;
        }

        *messages = staged;
        total_freed
    }
}

// =============================================================================
// Production wiring
// =============================================================================

/// The production cheap-pass pipeline, config-gated.
///
/// Single source for every turn driver that wants the deterministic pre-LLM
/// passes: the main runner (`harness_bridge::runner_impl`) and the subagent
/// spawner both call this instead of re-listing the stages. The list used to
/// live inline in the runner only, which is exactly why subagents ran with no
/// preflight at all — a second construction site had to re-derive it from
/// nothing.
///
/// All three stages share ONE config-derived gate: the preventive band just
/// below the LLM-compaction warning line. `FileOpSupersedeStage`'s own ratio is
/// overridden to that same value so its standalone gate cannot drift above a
/// custom warning threshold.
///
/// The cache guards ([`PreflightPipeline::with_cache_stability`]) are wired
/// from the same config: the boundary steps in [`DEFAULT_CUT_QUANTUM`]-message
/// increments and a pass must free [`DEFAULT_MIN_SAVINGS_RATIO`] of the budget
/// to be committed, both waived once the fill ratio reaches the warning line
/// that governs LLM compaction.
///
/// Ordering: `FileOpSupersedeStage` first so its stubs shrink the tool_result
/// bodies before the pruner and the image stripper see them. The three stages
/// are commutative for correctness (none touches the others' targets); the
/// order is for log readability and minor cache wins.
#[must_use]
pub fn default_pipeline(cfg: &super::ContextBudgetConfig) -> PreflightPipeline {
    use super::cheap_passes::{
        FileOpSupersedeStage, HistoricalImageStrippingStage, ToolResultPruningStage,
    };
    let preventive_floor = cfg.preventive_floor();
    let stages: Vec<Box<dyn PreflightStage>> = vec![
        Box::new(FileOpSupersedeStage::default().with_min_pressure_ratio(preventive_floor)),
        Box::new(ToolResultPruningStage::default()),
        Box::new(HistoricalImageStrippingStage),
    ];
    PreflightPipeline::new(stages)
        .with_min_pressure_ratio(preventive_floor)
        .with_cache_stability(
            DEFAULT_CUT_QUANTUM,
            DEFAULT_MIN_SAVINGS_RATIO,
            cfg.warning_threshold,
        )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock stage that returns a fixed number of freed tokens.
    struct MockStage {
        name: &'static str,
        tokens_to_free: usize,
    }

    #[async_trait]
    impl PreflightStage for MockStage {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn prepare(
            &self,
            _messages: &mut Vec<UnifiedMessage>,
            _pressure: &ContextPressure,
            _fresh_tail_count: usize,
        ) -> usize {
            self.tokens_to_free
        }
    }

    /// Mock stage that only frees tokens when pressure ratio exceeds a threshold.
    struct ThresholdStage {
        name: &'static str,
        threshold: f64,
        tokens_to_free: usize,
    }

    #[async_trait]
    impl PreflightStage for ThresholdStage {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn prepare(
            &self,
            _messages: &mut Vec<UnifiedMessage>,
            pressure: &ContextPressure,
            _fresh_tail_count: usize,
        ) -> usize {
            if pressure.ratio >= self.threshold {
                self.tokens_to_free
            } else {
                0
            }
        }
    }

    fn make_pressure(ratio: f64) -> ContextPressure {
        let budget = 10_000usize;
        let used = (budget as f64 * ratio) as usize;
        ContextPressure {
            used_tokens: used,
            budget_tokens: budget,
            ratio,
            overhead_tokens: 0,
            available_for_messages: budget,
        }
    }

    #[tokio::test]
    async fn empty_pipeline_frees_zero_tokens() {
        let pipeline = PreflightPipeline::empty();
        let mut msgs = vec![UnifiedMessage::user("hello")];
        let pressure = make_pressure(0.5);
        let freed = pipeline.run(&mut msgs, &pressure, 1).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn preventive_band_skips_all_stages_below_floor() {
        // A pipeline gated at 0.60 must free nothing below the floor (every
        // stage skipped) and run normally at/above it — the preventive band.
        let pipeline = PreflightPipeline::new(vec![Box::new(MockStage {
            name: "lossy",
            tokens_to_free: 500,
        })])
        .with_min_pressure_ratio(0.60);
        let mut msgs = vec![UnifiedMessage::user("test")];

        let calm = pipeline.run(&mut msgs, &make_pressure(0.50), 1).await;
        assert_eq!(
            calm, 0,
            "below the preventive floor the pipeline is a no-op"
        );

        let pressured = pipeline.run(&mut msgs, &make_pressure(0.70), 1).await;
        assert_eq!(pressured, 500, "at/above the floor stages run normally");
    }

    #[tokio::test]
    async fn default_pipeline_floor_is_always_on() {
        // `new` without a floor keeps the historical behaviour: stages run even
        // at very low pressure (floor defaults to 0.0).
        let pipeline = PreflightPipeline::new(vec![Box::new(MockStage {
            name: "always",
            tokens_to_free: 42,
        })]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let freed = pipeline.run(&mut msgs, &make_pressure(0.01), 1).await;
        assert_eq!(freed, 42);
    }

    #[tokio::test]
    async fn pipeline_runs_stages_in_order_and_sums_freed() {
        let pipeline = PreflightPipeline::new(vec![
            Box::new(MockStage {
                name: "stage_a",
                tokens_to_free: 100,
            }),
            Box::new(MockStage {
                name: "stage_b",
                tokens_to_free: 250,
            }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let pressure = make_pressure(0.8);
        let freed = pipeline.run(&mut msgs, &pressure, 1).await;
        assert_eq!(freed, 350);
    }

    #[tokio::test]
    async fn pipeline_respects_per_stage_pressure_thresholds() {
        let pipeline = PreflightPipeline::new(vec![
            Box::new(ThresholdStage {
                name: "low_threshold",
                threshold: 0.5,
                tokens_to_free: 100,
            }),
            Box::new(ThresholdStage {
                name: "high_threshold",
                threshold: 0.9,
                tokens_to_free: 200,
            }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];

        // Pressure at 0.7 — only the low-threshold stage should fire
        let pressure = make_pressure(0.7);
        let freed = pipeline.run(&mut msgs, &pressure, 1).await;
        assert_eq!(freed, 100, "only low_threshold stage should fire at 0.7");

        // Pressure at 0.95 — both stages should fire
        let pressure_high = make_pressure(0.95);
        let freed_high = pipeline.run(&mut msgs, &pressure_high, 1).await;
        assert_eq!(freed_high, 300, "both stages should fire at 0.95");
    }

    #[tokio::test]
    async fn pipeline_passes_fresh_tail_count_to_stages() {
        /// Stage that records the fresh_tail_count it received.
        struct RecordingStage;

        #[async_trait]
        impl PreflightStage for RecordingStage {
            fn name(&self) -> &'static str {
                "recording"
            }

            async fn prepare(
                &self,
                _messages: &mut Vec<UnifiedMessage>,
                _pressure: &ContextPressure,
                fresh_tail_count: usize,
            ) -> usize {
                // Return fresh_tail_count as "freed" so the test can verify it
                fresh_tail_count
            }
        }

        let pipeline = PreflightPipeline::new(vec![Box::new(RecordingStage)]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let pressure = make_pressure(0.5);
        let freed = pipeline.run(&mut msgs, &pressure, 42).await;
        assert_eq!(freed, 42, "stage should receive fresh_tail_count=42");
    }

    // =========================================================================
    // Cache-stability guards
    // =========================================================================

    use crate::providers::message::ContentBlock;

    /// Stage that actually rewrites history, so "committed" vs "discarded" is
    /// observable in `messages` and not just in the returned count.
    struct PruneStage {
        saving_per_msg: usize,
    }

    #[async_trait]
    impl PreflightStage for PruneStage {
        fn name(&self) -> &'static str {
            "prune"
        }

        async fn prepare(
            &self,
            messages: &mut Vec<UnifiedMessage>,
            _pressure: &ContextPressure,
            fresh_tail_count: usize,
        ) -> usize {
            if messages.len() <= fresh_tail_count {
                return 0;
            }
            let cut = messages.len() - fresh_tail_count;
            let mut freed = 0;
            for msg in messages.iter_mut().take(cut) {
                if let UnifiedMessage::User { content } = msg {
                    if let Some(ContentBlock::Text { text, .. }) = content.first_mut() {
                        if text.starts_with("big") {
                            *text = "[pruned]".to_string();
                            freed += self.saving_per_msg;
                        }
                    }
                }
            }
            freed
        }
    }

    /// Flatten a message's text blocks — the bytes a provider would hash.
    fn rendered(msg: &UnifiedMessage) -> String {
        let content = match msg {
            UnifiedMessage::User { content } | UnifiedMessage::Assistant { content } => content,
            UnifiedMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn history(len: usize) -> Vec<UnifiedMessage> {
        (0..len)
            .map(|i| UnifiedMessage::user(format!("big-{i}")))
            .collect()
    }

    /// `make_pressure` uses a 10_000-token budget, so a 0.02 ratio is a
    /// 200-token floor.
    fn guarded(saving_per_msg: usize, quantum: usize) -> PreflightPipeline {
        PreflightPipeline::new(vec![Box::new(PruneStage { saving_per_msg })])
            .with_cache_stability(quantum, 0.02, 0.70)
    }

    #[test]
    fn quantized_cut_steps_instead_of_sliding() {
        // Cut = len - fresh_tail, rounded down to a multiple of the quantum and
        // handed back as a tail count. 94, 95 → the same cut of 80.
        assert_eq!(quantized_tail(100, 6, 16), 20);
        assert_eq!(quantized_tail(101, 6, 16), 21);
        // 96 is itself a multiple — the step rolls over here, not before.
        assert_eq!(quantized_tail(102, 6, 16), 6);
        assert_eq!(quantized_tail(104, 6, 16), 8);
        // quantum <= 1 is the historical per-message slide.
        assert_eq!(quantized_tail(100, 6, 1), 6);
        assert_eq!(quantized_tail(100, 6, 0), 6);
        // Shorter than the tail: cut 0, so every message is protected.
        assert_eq!(quantized_tail(3, 6, 16), 3);
    }

    #[tokio::test]
    async fn a_pass_that_frees_less_than_the_floor_is_discarded() {
        // 10 prunable messages x 10 tokens = 100 freed, under the 200 floor:
        // not worth re-billing the whole message prefix at cache_creation.
        let pipeline = guarded(10, 1);
        let mut msgs = history(16);
        let freed = pipeline.run(&mut msgs, &make_pressure(0.50), 6).await;

        assert_eq!(freed, 0, "a discarded pass reports no savings");
        assert!(
            msgs.iter().all(|m| rendered(m).starts_with("big")),
            "a discarded pass must leave history byte-identical — its \
             candidates stay pending for a later, larger batch"
        );
    }

    #[tokio::test]
    async fn a_pass_that_clears_the_floor_is_committed() {
        // Same shape, 50 tokens each: 500 freed clears the 200 floor.
        let pipeline = guarded(50, 1);
        let mut msgs = history(16);
        let freed = pipeline.run(&mut msgs, &make_pressure(0.50), 6).await;

        assert_eq!(freed, 500);
        assert_eq!(rendered(&msgs[0]), "[pruned]");
        assert_eq!(rendered(&msgs[9]), "[pruned]");
        assert_eq!(rendered(&msgs[10]), "big-10", "the fresh tail is untouched");
    }

    #[tokio::test]
    async fn history_renders_identically_while_the_boundary_sits_in_one_step() {
        // THE invariant this guard exists for. Two consecutive turns whose cut
        // (94, 95) rounds to the same multiple of 16 must produce the same
        // bytes for every message they share — otherwise the provider reads a
        // different prefix and re-bills all of it at cache_creation.
        let pipeline = guarded(50, 16);

        let mut turn_1 = history(100);
        pipeline.run(&mut turn_1, &make_pressure(0.50), 6).await;

        let mut turn_2 = history(101);
        pipeline.run(&mut turn_2, &make_pressure(0.50), 6).await;

        for (i, (a, b)) in turn_1.iter().zip(turn_2.iter()).enumerate() {
            assert_eq!(
                rendered(a),
                rendered(b),
                "message {i} was rewritten between two turns of the same step"
            );
        }
        assert_eq!(rendered(&turn_1[79]), "[pruned]", "the step did prune");
        assert_eq!(rendered(&turn_1[80]), "big-80", "and stopped at the step");
    }

    #[tokio::test]
    async fn without_the_quantum_the_boundary_slides_and_breaks_the_prefix() {
        // Negative control for the test above: the historical per-message
        // slide rewrites one more message every turn, and that message sits
        // below all three message-level cache breakpoints.
        let pipeline = guarded(50, 1);

        let mut turn_1 = history(100);
        pipeline.run(&mut turn_1, &make_pressure(0.50), 6).await;

        let mut turn_2 = history(101);
        pipeline.run(&mut turn_2, &make_pressure(0.50), 6).await;

        assert_eq!(rendered(&turn_1[94]), "big-94");
        assert_eq!(
            rendered(&turn_2[94]),
            "[pruned]",
            "the sliding boundary rewrites a message the previous turn had left \
             alone — this is the per-turn prefix miss the quantum removes"
        );
    }

    #[tokio::test]
    async fn at_the_compaction_line_both_cache_guards_are_waived() {
        // 0.75 is at/above the 0.70 flush ratio: the compactor is about to
        // rewrite history anyway, so take every token — the 10-per-message
        // saving is under the floor and the cut ignores the quantum.
        let pipeline = guarded(10, 16);
        let mut msgs = history(100);
        let freed = pipeline.run(&mut msgs, &make_pressure(0.75), 6).await;

        assert_eq!(freed, 940, "every message outside the fresh tail is pruned");
        assert_eq!(rendered(&msgs[93]), "[pruned]");
        assert_eq!(rendered(&msgs[94]), "big-94", "the fresh tail still holds");
    }

    #[tokio::test]
    async fn the_preventive_floor_still_wins_over_the_waiver() {
        // Ordering check: below the preventive band nothing runs at all, even
        // though the flush ratio would have waived the cache guards.
        let pipeline = guarded(50, 16).with_min_pressure_ratio(0.60);
        let mut msgs = history(100);
        let freed = pipeline.run(&mut msgs, &make_pressure(0.50), 6).await;

        assert_eq!(freed, 0);
        assert!(msgs.iter().all(|m| rendered(m).starts_with("big")));
    }
}
