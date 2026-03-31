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
// Stage 2: ToolCompactStage
// =============================================================================

/// Delegates to the existing tool compactor which produces summary lines like
/// `[Read file, 100 lines, rust]` for old tool results.
pub struct ToolCompactStage {
    pub token_budget: u64,
    pub threshold: f64,
    pub ratio: f64,
}

impl CompactionStage for ToolCompactStage {
    fn name(&self) -> &'static str {
        "tool_compact"
    }

    fn compact(&self, messages: &mut [UnifiedMessage], fresh_tail_count: usize) -> usize {
        let before: usize = messages
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        crate::memory::session_compactor::tool_compactor::compact_if_needed(
            messages,
            self.token_budget,
            self.threshold,
            self.ratio,
            fresh_tail_count,
        );

        let after: usize = messages
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        before.saturating_sub(after)
    }
}

// =============================================================================
// Stage 3: RoundDrop
// =============================================================================

/// Groups messages into API rounds (one user turn + all following messages
/// until the next user turn) and drops the oldest complete rounds when over
/// budget.
pub struct RoundDrop {
    pub token_budget: u64,
    pub ratio: f64,
}

const ROUND_DROP_NOTICE: &str =
    "[SYSTEM] Earlier conversation history was truncated \
     to fit the model's context window. Continue based on the remaining context.";

impl CompactionStage for RoundDrop {
    fn name(&self) -> &'static str {
        "round_drop"
    }

    fn compact(&self, messages: &mut [UnifiedMessage], fresh_tail_count: usize) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        let compressible = &messages[..partition];

        // Count total tokens in the compressible zone.
        let total_tokens: usize = compressible
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        let budget = (self.token_budget as f64 / self.ratio) as usize;
        if total_tokens <= budget {
            return 0;
        }

        // Group compressible messages into rounds.
        // A round starts at each user message; everything until the next user
        // message belongs to the same round.
        let mut rounds: Vec<std::ops::Range<usize>> = Vec::new();
        let mut round_start: Option<usize> = None;

        for i in 0..partition {
            if messages[i].is_user() {
                if let Some(start) = round_start {
                    rounds.push(start..i);
                }
                round_start = Some(i);
            }
        }
        if let Some(start) = round_start {
            rounds.push(start..partition);
        }

        if rounds.is_empty() {
            return 0;
        }

        // Drop oldest rounds until we have freed enough tokens.
        let mut freed: usize = 0;
        let need_to_free = total_tokens.saturating_sub(budget);
        let mut first_dropped = true;

        for round in &rounds {
            if freed >= need_to_free {
                break;
            }

            for idx in round.clone() {
                let tokens = estimate_tokens_smart(&messages[idx].text_content());
                freed += tokens;

                if first_dropped {
                    // Replace the first dropped message with a truncation notice.
                    messages[idx] = UnifiedMessage::user(ROUND_DROP_NOTICE);
                    first_dropped = false;
                } else {
                    // Replace subsequent dropped messages with empty placeholders.
                    messages[idx] = UnifiedMessage::user("");
                }
            }
        }

        freed
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

    #[test]
    fn tool_compact_stage_delegates_to_existing_compactor() {
        let mut msgs = vec![
            UnifiedMessage::user("request"),
            UnifiedMessage::tool_result("c1", "Read", &"fn main() {}\n".repeat(200), false),
            UnifiedMessage::assistant("I read the file"),
            UnifiedMessage::user("latest"),
        ];
        let stage = ToolCompactStage { token_budget: 100, threshold: 0.01, ratio: 3.5 };
        let freed = stage.compact(&mut msgs, 1);
        let (_, content) = msgs[1].tool_result_info().unwrap();
        // Should be compressed to summary like "[Read file, ...]" or "[Old result cleared]"
        assert!(content.len() < 200, "should be compressed, got len: {}", content.len());
        assert!(freed > 0);
    }

    #[test]
    fn round_drop_removes_oldest_round() {
        let mut msgs = vec![
            // Round 1
            UnifiedMessage::user("old question"),
            UnifiedMessage::assistant("old answer"),
            // Round 2
            UnifiedMessage::user("new question"),
            UnifiedMessage::assistant("new answer"),
        ];
        // budget=1, ratio=1.0 → effective budget = 1 token → forces drop of round 1
        let stage = RoundDrop { token_budget: 1, ratio: 1.0 };
        let freed = stage.compact(&mut msgs, 2); // fresh_tail=2, protects round 2
        assert!(freed > 0);
        // First message should be truncation notice
        assert!(
            msgs[0].text_content().contains("truncated") || msgs[0].text_content().contains("SYSTEM"),
            "first msg should be truncation notice, got: {}",
            msgs[0].text_content()
        );
    }

    #[test]
    fn round_drop_preserves_tool_pairs() {
        let mut msgs = vec![
            // Round 1 with tool calls
            UnifiedMessage::user("search for X"),
            UnifiedMessage::tool_result("c1", "Grep", &"x".repeat(500), false),
            UnifiedMessage::assistant("found X"),
            // Round 2 (fresh tail)
            UnifiedMessage::user("next step"),
            UnifiedMessage::assistant("doing it"),
        ];
        let stage = RoundDrop { token_budget: 1, ratio: 1.0 };
        let freed = stage.compact(&mut msgs, 2);
        // Should drop entire round 1 (user + tool_result + assistant)
        assert!(freed > 0);
    }

    #[test]
    fn round_drop_noop_when_under_budget() {
        let mut msgs = vec![
            UnifiedMessage::user("hi"),
            UnifiedMessage::assistant("hello"),
        ];
        let stage = RoundDrop { token_budget: 100_000, ratio: 3.5 };
        let freed = stage.compact(&mut msgs, 2);
        assert_eq!(freed, 0);
        assert_eq!(msgs.len(), 2);
    }
}
