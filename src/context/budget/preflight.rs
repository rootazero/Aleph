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
        }
    }

    /// Create an empty pipeline (no-op).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            stages: Vec::new(),
            min_pressure_ratio: 0.0,
        }
    }

    /// Set the preventive-band floor: below this fill ratio
    /// [`run`](Self::run) skips every stage and frees nothing.
    #[must_use]
    pub fn with_min_pressure_ratio(mut self, ratio: f64) -> Self {
        self.min_pressure_ratio = ratio;
        self
    }

    /// Run all stages in order, returning the total tokens freed.
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
    PreflightPipeline::new(stages).with_min_pressure_ratio(preventive_floor)
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
}
