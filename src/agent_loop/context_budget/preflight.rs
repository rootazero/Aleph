//! PreflightPipeline — async stage execution before the main compaction pipeline.
//!
//! Unlike the synchronous `CompactionStage` trait in `pipeline.rs`, preflight
//! stages are async because later implementations (e.g. Autocompact) will call
//! LLMs to summarize or reorganize context.

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
}

impl PreflightPipeline {
    /// Create a new pipeline with the given ordered stages.
    pub fn new(stages: Vec<Box<dyn PreflightStage>>) -> Self {
        Self { stages }
    }

    /// Create an empty pipeline (no-op).
    pub fn empty() -> Self {
        Self { stages: Vec::new() }
    }

    /// Run all stages in order, returning the total tokens freed.
    pub async fn run(
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
