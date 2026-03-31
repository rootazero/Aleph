//! CompactionPipeline — ordered strategy execution.

use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::agent_loop::tool::ToolDefinition;
use super::pressure::{PressureSensor, estimate_tokens_smart};
use super::ContextPressure;
use crate::memory::session_compactor::context_window::{
    is_tool_result_consumed, partition_fresh_tail,
};

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
// Stage 0: ImageStripper
// =============================================================================

/// Replaces image content blocks in compressible messages with a lightweight
/// text marker, freeing the token budget occupied by base64-encoded image data.
pub struct ImageStripper;

const IMAGE_TOKEN_ESTIMATE: usize = 2000;
const IMAGE_MARKER: &str = "[image, ~2000 tokens]";

impl CompactionStage for ImageStripper {
    fn name(&self) -> &'static str {
        "image_stripper"
    }

    fn compact(&self, messages: &mut [UnifiedMessage], fresh_tail_count: usize) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        let mut total_freed: usize = 0;

        for msg in messages[..partition].iter_mut() {
            for block in msg.content_blocks_mut() {
                if matches!(block, ContentBlock::Image { .. }) {
                    *block = ContentBlock::Text {
                        text: IMAGE_MARKER.to_string(),
                    };
                    total_freed += IMAGE_TOKEN_ESTIMATE;
                }
            }
        }

        total_freed
    }
}

// =============================================================================
// Stage 1: MicroCompact
// =============================================================================

/// Clears the text of old tool results that have already been consumed by the
/// LLM (i.e. an assistant message follows them), replacing them with a small
/// marker to reclaim token budget.
pub struct MicroCompact;

const CLEARED_MARKER: &str = "[Old result cleared]";

impl CompactionStage for MicroCompact {
    fn name(&self) -> &'static str {
        "micro_compact"
    }

    fn compact(&self, messages: &mut [UnifiedMessage], fresh_tail_count: usize) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);

        // Collect candidate indices: tool results in the compressible zone that
        // have been consumed (an assistant turn follows them).
        let candidates: Vec<usize> = (0..partition)
            .filter(|&i| {
                messages[i].is_tool_result()
                    && is_tool_result_consumed(messages, i)
            })
            .collect();

        let mut total_freed: usize = 0;

        for idx in candidates {
            let old_content = match messages[idx].tool_result_info() {
                Some((_, content)) => content,
                None => continue,
            };

            // Skip if already compacted.
            if old_content == CLEARED_MARKER {
                continue;
            }

            let old_tokens = estimate_tokens_smart(&old_content);
            let marker_tokens = estimate_tokens_smart(CLEARED_MARKER);
            messages[idx].replace_tool_result_content(CLEARED_MARKER.to_string());
            // Count at least 1 freed token — replacing any content with the
            // marker represents a real compaction action even if the content
            // was already short.
            let saved = old_tokens.saturating_sub(marker_tokens).max(1);
            total_freed += saved;
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

    #[test]
    fn image_stripper_replaces_image_blocks() {
        use crate::providers::message::ContentBlock;
        let mut msgs = vec![
            UnifiedMessage::user_with_content(vec![
                ContentBlock::Image {
                    data: "base64data".repeat(100),
                    mime_type: "image/png".into(),
                },
            ]),
            UnifiedMessage::assistant("I see the image"),
            UnifiedMessage::user("latest question"),
        ];
        let stage = ImageStripper;
        let freed = stage.compact(&mut msgs, 1); // fresh_tail=1, protects last msg
        let content = msgs[0].text_content();
        assert!(content.contains("[image"), "image should be replaced, got: {content}");
        assert!(freed > 0, "should have freed tokens");
    }

    #[test]
    fn image_stripper_preserves_fresh_tail_images() {
        use crate::providers::message::ContentBlock;
        let mut msgs = vec![
            UnifiedMessage::user("old text"),
            UnifiedMessage::user_with_content(vec![
                ContentBlock::Image {
                    data: "base64data".repeat(100),
                    mime_type: "image/png".into(),
                },
            ]),
        ];
        let stage = ImageStripper;
        let freed = stage.compact(&mut msgs, 2); // all in fresh tail
        assert_eq!(freed, 0, "should not touch fresh tail images");
    }

    #[test]
    fn microcompact_clears_old_consumed_tool_results() {
        let mut msgs = vec![
            UnifiedMessage::user("do something"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(2000), false),
            UnifiedMessage::assistant("I processed it"),
            UnifiedMessage::user("latest"),
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 1);
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert_eq!(content, "[Old result cleared]");
        assert!(freed > 0);
    }

    #[test]
    fn microcompact_preserves_unconsumed_tool_results() {
        let mut msgs = vec![
            UnifiedMessage::user("do something"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(2000), false),
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 0);
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert_eq!(content, "x".repeat(2000));
        assert_eq!(freed, 0);
    }

    #[test]
    fn microcompact_preserves_fresh_tail() {
        let mut msgs = vec![
            UnifiedMessage::user("old"),
            UnifiedMessage::tool_result("c1", "Bash", "old output", false),
            UnifiedMessage::assistant("old reply"),
            UnifiedMessage::user("new"),
            UnifiedMessage::tool_result("c2", "Read", &"y".repeat(2000), false),
            UnifiedMessage::assistant("new reply"),
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 3); // last 3 are fresh
        let (_, content) = msgs[4].tool_result_info().unwrap();
        assert_eq!(content, "y".repeat(2000)); // fresh tail untouched
        let (_, old) = msgs[1].tool_result_info().unwrap();
        assert_eq!(old, "[Old result cleared]"); // old one cleared
        assert!(freed > 0);
    }
}
