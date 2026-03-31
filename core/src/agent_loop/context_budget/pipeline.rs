//! CompactionPipeline — ordered strategy execution.

use crate::providers::message::UnifiedMessage;
use crate::agent_loop::tool::ToolDefinition;
use super::pressure::PressureSensor;
use super::ContextPressure;

// =============================================================================
// CompactionStage trait
// =============================================================================

/// A single compaction strategy that can be composed into a pipeline.
pub trait CompactionStage: Send + Sync {
    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Attempt to free tokens from the message list.
    ///
    /// `fresh_tail_count` messages at the tail are protected and must not be
    /// modified. Returns the number of tokens freed (estimated).
    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize;
}

// =============================================================================
// PipelineResult
// =============================================================================

/// Result of a full pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Pressure measured before any stages ran.
    pub pressure_before: ContextPressure,
    /// Pressure measured after all stages completed (or after early exit).
    pub pressure_after: ContextPressure,
    /// Total tokens freed across all stages that ran.
    pub tokens_freed: usize,
    /// (stage_name, tokens_freed) for each stage that was executed.
    pub stages_run: Vec<(&'static str, usize)>,
}

// =============================================================================
// CompactionPipeline
// =============================================================================

/// Executes an ordered list of `CompactionStage`s, stopping early when
/// context pressure drops below the target ratio.
pub struct CompactionPipeline {
    stages: Vec<Box<dyn CompactionStage>>,
}

impl CompactionPipeline {
    /// Create a new pipeline with the given ordered stages.
    pub fn new(stages: Vec<Box<dyn CompactionStage>>) -> Self {
        Self { stages }
    }

    /// Run the pipeline against the given message list.
    ///
    /// Stages are executed in order. Before each stage the sensor re-measures
    /// pressure; if pressure has fallen below `target_ratio` the pipeline stops
    /// early (the stage is *not* run).
    ///
    /// Returns a `PipelineResult` describing what happened.
    pub fn run(
        &self,
        messages: &mut [UnifiedMessage],
        sensor: &PressureSensor,
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
        target_ratio: f64,
        fresh_tail_count: usize,
    ) -> PipelineResult {
        let pressure_before = sensor.measure(messages, system_prompt, tool_defs, token_budget);

        let mut stages_run: Vec<(&'static str, usize)> = Vec::new();
        let mut total_freed: usize = 0;

        for stage in &self.stages {
            // Check pressure before running this stage.
            let current = sensor.measure(messages, system_prompt, tool_defs, token_budget);
            if current.ratio < target_ratio {
                tracing::info!(
                    target: "compaction_pipeline",
                    stage = stage.name(),
                    ratio = current.ratio,
                    target = target_ratio,
                    "Pressure below target — stopping pipeline early"
                );
                break;
            }

            let freed = stage.compact(messages, fresh_tail_count);
            total_freed += freed;
            stages_run.push((stage.name(), freed));

            tracing::info!(
                target: "compaction_pipeline",
                stage = stage.name(),
                tokens_freed = freed,
                total_freed,
                "Compaction stage completed"
            );
        }

        let pressure_after = sensor.measure(messages, system_prompt, tool_defs, token_budget);

        PipelineResult {
            pressure_before,
            pressure_after,
            tokens_freed: total_freed,
            stages_run,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use super::super::pressure::PressureSensor;

    struct MockStage {
        name: &'static str,
        tokens_to_free: usize,
    }

    impl CompactionStage for MockStage {
        fn name(&self) -> &'static str {
            self.name
        }

        fn compact(&self, _messages: &mut [UnifiedMessage], _fresh_tail_count: usize) -> usize {
            self.tokens_to_free
        }
    }

    #[test]
    fn pipeline_runs_stages_in_order() {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "stage_a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "stage_b", tokens_to_free: 200 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let sensor = PressureSensor::new(3.5);
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 100, 0.0, 2);
        assert_eq!(result.stages_run.len(), 2);
        assert_eq!(result.stages_run[0].0, "stage_a");
        assert_eq!(result.stages_run[1].0, "stage_b");
    }

    #[test]
    fn pipeline_stops_early_when_pressure_below_target() {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "stage_a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "stage_b", tokens_to_free: 200 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("x")];
        let sensor = PressureSensor::new(3.5);
        // Budget=10000, messages tiny → ratio ≈ 0.0, target=0.70 → already below
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 10_000, 0.70, 2);
        assert_eq!(result.stages_run.len(), 0, "should skip all stages when already under target");
    }

    #[test]
    fn pipeline_result_tracks_total_freed() {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "b", tokens_to_free: 250 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let sensor = PressureSensor::new(3.5);
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 100, 0.0, 2);
        assert_eq!(result.tokens_freed, 350);
    }
}
