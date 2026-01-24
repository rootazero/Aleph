//! Compaction trigger for SessionCompactor integration

use std::sync::Arc;

use crate::event::{
    AetherEvent, EventBus, LoopState as EventLoopState, StopReason, TokenUsage, ToolCallResult,
};

/// Helper for triggering compaction checks at key points in the agent loop.
///
/// This struct encapsulates the event emission logic for SessionCompactor
/// integration, following OpenCode's pattern of checking for context overflow
/// before each iteration and after tool execution.
///
/// # Event Flow
///
/// ```text
/// AgentLoop                    EventBus                 SessionCompactor
///     │                           │                           │
///     │── LoopContinue ──────────>│────────────────────────>│
///     │                           │                  (check overflow)
///     │                           │                           │
///     │── ToolCallCompleted ─────>│────────────────────────>│
///     │                           │                   (prune check)
///     │                           │                           │
///     │── LoopStop ──────────────>│────────────────────────>│
///     │                           │                (final cleanup)
/// ```
pub struct CompactionTrigger {
    event_bus: Arc<EventBus>,
}

impl CompactionTrigger {
    /// Create a new CompactionTrigger with the given EventBus
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }

    /// Emit LoopContinue event before each iteration
    ///
    /// This triggers SessionCompactor to check for overflow and
    /// compact the session if needed. Should be called at the start
    /// of each loop iteration (except the first).
    pub async fn emit_loop_continue(
        &self,
        session_id: &str,
        iteration: u32,
        total_tokens: u64,
        last_tool: Option<String>,
        model: &str,
    ) {
        let loop_state = EventLoopState {
            session_id: session_id.to_string(),
            iteration,
            total_tokens,
            last_tool,
            model: model.to_string(),
        };

        tracing::debug!(
            session_id = %session_id,
            iteration = iteration,
            total_tokens = total_tokens,
            "Emitting LoopContinue for compaction check"
        );

        self.event_bus
            .publish(AetherEvent::LoopContinue(loop_state))
            .await;
    }

    /// Emit ToolCallCompleted event after tool execution
    ///
    /// This triggers SessionCompactor to check if pruning is needed.
    #[allow(clippy::too_many_arguments)]
    pub async fn emit_tool_completed(
        &self,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        output: &str,
        started_at: i64,
        completed_at: i64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let result = ToolCallResult {
            call_id: call_id.to_string(),
            tool: tool_name.to_string(),
            input,
            output: output.to_string(),
            started_at,
            completed_at,
            token_usage: TokenUsage {
                input_tokens,
                output_tokens,
            },
            session_id: Some(session_id.to_string()),
        };

        tracing::debug!(
            session_id = %session_id,
            tool = %tool_name,
            "Emitting ToolCallCompleted for pruning check"
        );

        self.event_bus
            .publish(AetherEvent::ToolCallCompleted(result))
            .await;
    }

    /// Emit LoopStop event when the session ends
    ///
    /// This triggers final cleanup and pruning by SessionCompactor.
    pub async fn emit_loop_stop(&self, reason: StopReason) {
        tracing::debug!(
            reason = ?reason,
            "Emitting LoopStop for final cleanup"
        );

        self.event_bus
            .publish(AetherEvent::LoopStop(reason))
            .await;
    }
}

/// Optional wrapper for CompactionTrigger that handles the None case gracefully
pub struct OptionalCompactionTrigger {
    inner: Option<CompactionTrigger>,
}

impl OptionalCompactionTrigger {
    /// Create a new OptionalCompactionTrigger
    pub fn new(event_bus: Option<Arc<EventBus>>) -> Self {
        Self {
            inner: event_bus.map(CompactionTrigger::new),
        }
    }

    /// Emit LoopContinue if trigger is available
    pub async fn emit_loop_continue(
        &self,
        session_id: &str,
        iteration: u32,
        total_tokens: u64,
        last_tool: Option<String>,
        model: &str,
    ) {
        if let Some(ref trigger) = self.inner {
            trigger
                .emit_loop_continue(session_id, iteration, total_tokens, last_tool, model)
                .await;
        }
    }

    /// Emit ToolCallCompleted if trigger is available
    #[allow(clippy::too_many_arguments)]
    pub async fn emit_tool_completed(
        &self,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        output: &str,
        started_at: i64,
        completed_at: i64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if let Some(ref trigger) = self.inner {
            trigger
                .emit_tool_completed(
                    session_id,
                    call_id,
                    tool_name,
                    input,
                    output,
                    started_at,
                    completed_at,
                    input_tokens,
                    output_tokens,
                )
                .await;
        }
    }

    /// Emit LoopStop if trigger is available
    pub async fn emit_loop_stop(&self, reason: StopReason) {
        if let Some(ref trigger) = self.inner {
            trigger.emit_loop_stop(reason).await;
        }
    }
}
