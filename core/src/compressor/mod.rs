//! Context Compressor for Agent Loop
//!
//! This module provides intelligent compression of conversation history
//! to manage token usage in long-running agent sessions.
//!
//! # Compression Strategy
//!
//! ```text
//! Steps 1-5: Full detail retained
//! Step 6:    Trigger compression
//!            Steps 1-3 → Summary
//!            Steps 4-6 → Full detail (sliding window)
//! Step 9:    Trigger compression again
//!            Previous summary + Steps 4-6 → Updated summary
//!            Steps 7-9 → Full detail
//! ```
//!
//! # Key Information Preserved
//!
//! - File changes (paths, create/modify/delete)
//! - Important tool outputs (errors, search results)
//! - User decisions
//! - Current state description
//!
//! # Context Statistics
//!
//! The module also provides context usage tracking via `ContextStats`:
//!
//! ```rust,ignore
//! let stats = ContextStats::from_state(&loop_state, max_tokens);
//! println!("{}", stats.summary());
//! if stats.is_critical() {
//!     println!("Warning: context is almost full!");
//! }
//! ```

pub mod context_stats;
pub mod smart_compactor;
pub mod smart_strategy;
pub mod strategy;
pub mod tool_truncator;
pub mod turn_protector;

#[cfg(test)]
mod tests_integration;

use crate::sync_primitives::Arc;

pub use context_stats::{CompressionFocus, ContextStats, UsageBreakdown, WarningLevel};
pub use smart_compactor::{CompactionResult, SmartCompactor};
pub use smart_strategy::{CompactionAction, SmartCompactionStrategy};
pub use strategy::{CompressionPrompt, KeyInfo, KeyInfoExtractor, RuleBasedStrategy};
pub use tool_truncator::{ToolTruncator, TruncatedOutput};
pub use turn_protector::TurnProtector;

use crate::agent_loop::{CompressionConfig, CompressedHistory, CompressorTrait, LoopState, LoopStep};
use crate::error::Result;
use crate::providers::AiProvider;

/// Generates the system prompt injected before context compaction
/// to give the agent a chance to persist important information to long-term memory.
pub struct PreCompactionPrompt;

impl PreCompactionPrompt {
    /// Build the flush prompt that instructs the agent to save key facts
    /// before context compaction discards older turns.
    pub fn build() -> String {
        concat!(
            "SYSTEM: Your conversation context is about to be compacted to save space. ",
            "Before this happens, review the recent conversation and use the memory_store tool ",
            "to persist any important information that should be remembered long-term. ",
            "Focus on: user preferences, decisions made, key facts learned, and task progress. ",
            "Do NOT store trivial greetings or small talk. ",
            "After storing, respond with only: [memory_flush_complete]"
        )
        .to_string()
    }
}

/// Context compressor for managing conversation history
///
/// The compressor monitors session state and compresses history
/// when it exceeds configured thresholds, preserving key information
/// while reducing token usage.
pub struct ContextCompressor {
    /// AI provider for LLM-based compression (optional)
    provider: Option<Arc<dyn AiProvider>>,
    /// Compression configuration
    config: CompressionConfig,
}

impl ContextCompressor {
    /// Create a new compressor with LLM support
    pub fn new(provider: Arc<dyn AiProvider>, config: CompressionConfig) -> Self {
        Self {
            provider: Some(provider),
            config,
        }
    }

    /// Create a compressor that uses rule-based compression only
    pub fn rule_based(config: CompressionConfig) -> Self {
        Self {
            provider: None,
            config,
        }
    }

    /// Get compression configuration
    pub fn config(&self) -> &CompressionConfig {
        &self.config
    }

    /// Check if state needs compression
    pub fn needs_compression(&self, state: &LoopState) -> bool {
        let uncompressed_steps = state.steps.len().saturating_sub(state.compressed_until_step);
        uncompressed_steps > self.config.compress_after_steps + self.config.recent_window_size
    }

    /// Get steps that need to be compressed
    fn get_steps_to_compress<'a>(&self, steps: &'a [LoopStep]) -> &'a [LoopStep] {
        if steps.len() <= self.config.recent_window_size {
            &[]
        } else {
            &steps[..steps.len() - self.config.recent_window_size]
        }
    }

    /// Compress steps using LLM
    async fn compress_with_llm(
        &self,
        provider: &dyn AiProvider,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<String> {
        let prompt = CompressionPrompt::build(
            current_summary,
            steps,
            self.config.target_summary_tokens,
        );

        let system_prompt = "You are a helpful assistant that summarizes conversation history concisely while preserving key information.";

        provider
            .process(&prompt, Some(system_prompt))
            .await
    }

    /// Compress steps using rule-based strategy
    fn compress_with_rules(&self, steps: &[LoopStep], current_summary: &str) -> String {
        RuleBasedStrategy::compress(steps, current_summary)
    }

    /// Execute compression
    async fn do_compress(
        &self,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory> {
        let steps_to_compress = self.get_steps_to_compress(steps);

        if steps_to_compress.is_empty() {
            return Ok(CompressedHistory {
                summary: current_summary.to_string(),
                compressed_count: 0,
            });
        }

        let summary = match &self.provider {
            Some(provider) if self.config.target_summary_tokens > 0 => {
                // Try LLM compression, fall back to rules on failure
                match self
                    .compress_with_llm(provider.as_ref(), steps_to_compress, current_summary)
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("LLM compression failed, using rule-based: {}", e);
                        self.compress_with_rules(steps_to_compress, current_summary)
                    }
                }
            }
            _ => self.compress_with_rules(steps_to_compress, current_summary),
        };

        Ok(CompressedHistory {
            summary,
            compressed_count: steps_to_compress.len(),
        })
    }
}

#[async_trait::async_trait]
impl CompressorTrait for ContextCompressor {
    fn should_compress(&self, state: &LoopState) -> bool {
        self.needs_compression(state)
    }

    async fn compress(
        &self,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory> {
        self.do_compress(steps, current_summary).await
    }
}

/// No-op compressor for testing
pub struct NoOpCompressor;

#[async_trait::async_trait]
impl CompressorTrait for NoOpCompressor {
    fn should_compress(&self, _state: &LoopState) -> bool {
        false
    }

    async fn compress(
        &self,
        _steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory> {
        Ok(CompressedHistory {
            summary: current_summary.to_string(),
            compressed_count: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{Action, ActionResult, Decision, RequestContext, Thinking};
    use serde_json::json;

    fn create_test_step(id: usize) -> LoopStep {
        LoopStep {
            step_id: id,
            observation_summary: String::new(),
            thinking: Thinking {
                reasoning: Some(format!("Step {} reasoning", id)),
                decision: Decision::Complete {
                    summary: "done".to_string(),
                },
                structured: None,
                tokens_used: None,
                tool_call_id: None,
            },
            action: Action::ToolCall {
                tool_name: format!("tool_{}", id),
                arguments: json!({}),
            },
            result: ActionResult::ToolSuccess {
                output: json!({"result": "ok"}),
                duration_ms: 100,
            },
            tokens_used: 100,
            duration_ms: 100,
        }
    }

    #[test]
    fn test_pre_compaction_prompt_contains_required_elements() {
        let prompt = PreCompactionPrompt::build();
        assert!(prompt.contains("memory_store"));
        assert!(prompt.contains("compacted"));
        assert!(prompt.contains("memory_flush_complete"));
    }

    #[test]
    fn test_needs_compression() {
        let config = CompressionConfig {
            compress_after_steps: 3,
            recent_window_size: 2,
            target_summary_tokens: 500,
            preserve_tool_outputs: true,
        };

        let compressor = ContextCompressor::rule_based(config);

        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        // With only 3 steps, no compression needed
        for i in 0..3 {
            state.steps.push(create_test_step(i));
        }
        assert!(!compressor.needs_compression(&state));

        // With 6 steps (> 3 + 2), compression needed
        for i in 3..6 {
            state.steps.push(create_test_step(i));
        }
        assert!(compressor.needs_compression(&state));
    }

    #[tokio::test]
    async fn test_rule_based_compression() {
        let config = CompressionConfig {
            compress_after_steps: 2,
            recent_window_size: 2,
            target_summary_tokens: 500,
            preserve_tool_outputs: true,
        };

        let compressor = ContextCompressor::rule_based(config);

        let steps: Vec<LoopStep> = (0..5).map(create_test_step).collect();

        let result = compressor.compress(&steps, "").await.unwrap();

        // Should compress first 3 steps (5 - 2 window size)
        assert_eq!(result.compressed_count, 3);
        assert!(!result.summary.is_empty());
    }

    #[tokio::test]
    async fn test_no_op_compressor() {
        let compressor = NoOpCompressor;

        let state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        assert!(!compressor.should_compress(&state));

        let result = compressor.compress(&[], "existing summary").await.unwrap();
        assert_eq!(result.summary, "existing summary");
        assert_eq!(result.compressed_count, 0);
    }

    #[test]
    fn test_get_steps_to_compress() {
        let config = CompressionConfig {
            compress_after_steps: 2,
            recent_window_size: 3,
            target_summary_tokens: 500,
            preserve_tool_outputs: true,
        };

        let compressor = ContextCompressor::rule_based(config);

        // With 5 steps and window size 3, should compress 2
        let steps: Vec<LoopStep> = (0..5).map(create_test_step).collect();
        let to_compress = compressor.get_steps_to_compress(&steps);
        assert_eq!(to_compress.len(), 2);

        // With only 2 steps, nothing to compress
        let steps: Vec<LoopStep> = (0..2).map(create_test_step).collect();
        let to_compress = compressor.get_steps_to_compress(&steps);
        assert!(to_compress.is_empty());
    }
}
