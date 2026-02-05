//! Trait abstractions for agent loop components

use crate::agent_loop::{Action, ActionResult, LoopState, LoopStep, Thinking};
use crate::agents::thinking::ThinkLevel;
use crate::error::Result;

/// Thinker trait - abstraction for the thinking layer
///
/// This trait is implemented by the Thinker module to provide
/// LLM-based decision making.
#[async_trait::async_trait]
pub trait ThinkerTrait: Send + Sync {
    /// Think and produce a decision using the configured thinking level
    async fn think(
        &self,
        state: &LoopState,
        tools: &[crate::dispatcher::UnifiedTool],
    ) -> Result<Thinking>;

    /// Think with a specific thinking level override
    ///
    /// Used by the fallback mechanism to retry with lower thinking levels.
    /// Default implementation ignores the level and calls think().
    async fn think_with_level(
        &self,
        state: &LoopState,
        tools: &[crate::dispatcher::UnifiedTool],
        _level: ThinkLevel,
    ) -> Result<Thinking> {
        self.think(state, tools).await
    }

    /// Get the current thinking level
    fn current_think_level(&self) -> ThinkLevel {
        ThinkLevel::default()
    }

    /// Think using semantically retrieved tools via HydrationPipeline
    ///
    /// This method uses pre-computed hydration results from semantic tool retrieval
    /// instead of keyword-based filtering. The HydrationResult contains tools
    /// classified by confidence level:
    /// - full_schema_tools: High confidence, include full JSON schema
    /// - summary_tools: Medium confidence, include description only
    /// - indexed_tool_names: Low confidence, just names for reference
    ///
    /// Default implementation falls back to keyword-based thinking with all tools.
    async fn think_with_hydration(
        &self,
        state: &LoopState,
        _hydration: &crate::dispatcher::tool_index::HydrationResult,
        tools: &[crate::dispatcher::UnifiedTool],
        level: ThinkLevel,
    ) -> Result<Thinking> {
        // Default: fall back to standard tool filtering
        self.think_with_level(state, tools, level).await
    }
}

/// Action Executor trait - abstraction for the execution layer
///
/// This trait is implemented by the Executor module to execute
/// individual actions in the agent loop (observe-think-act cycle).
///
/// Note: This is distinct from:
/// - `dispatcher::executor::TaskExecutor` - for task-type specific execution
/// - `dispatcher::scheduler::GraphTaskExecutor` - for DAG node execution
#[async_trait::async_trait]
pub trait ActionExecutor: Send + Sync {
    /// Execute a single action
    async fn execute(&self, action: &Action) -> ActionResult;
}

/// Deprecated alias for backward compatibility
#[deprecated(since = "0.2.0", note = "Use ActionExecutor instead")]
#[allow(dead_code)]
pub type ExecutorTrait = dyn ActionExecutor;

/// Compressor trait - abstraction for context compression
///
/// This trait is implemented by the ContextCompressor module
/// to compress history for long-running sessions.
#[async_trait::async_trait]
pub trait CompressorTrait: Send + Sync {
    /// Check if compression is needed
    fn should_compress(&self, state: &LoopState) -> bool;

    /// Compress history and return summary
    async fn compress(
        &self,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory>;
}

/// Result of compression
#[derive(Debug, Clone)]
pub struct CompressedHistory {
    /// New summary text
    pub summary: String,
    /// Number of steps that were compressed
    pub compressed_count: usize,
}
