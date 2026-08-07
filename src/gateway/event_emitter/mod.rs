//! Event Emitter for Streaming
//!
//! Provides the `EventEmitter` trait for emitting real-time streaming events
//! from the agent loop to connected WebSocket clients.

pub mod artifact_ping;
mod impls;
mod instant_buffer;
pub mod origin_fanout;
pub mod redacting;
pub mod team_fanout;
mod types;

#[cfg(test)]
mod tests;

// Re-export all public types
pub use types::{
    ConfidenceLevel, OutputMode, ReasoningStepType, RunSummary, StreamEvent, ToolErrorItem,
    ToolResult, ToolSummaryItem, UncertaintyAction,
};

pub use impls::{CollectingEventEmitter, DynEventEmitter, GatewayEventEmitter, NoOpEventEmitter};
pub use instant_buffer::InstantBufferingEmitter;
pub use redacting::RedactingEmitter;

/// The gateway event bus injected once at boot (`team_fanout::set_team_event_bus`).
/// The slot is not team-specific: any out-of-band producer that must reach the
/// Panel mid-run — team fan-out, `ask_user` — publishes through it. `None` in
/// contexts that never built a gateway (CLI subcommands, unit tests).
pub use team_fanout::team_event_bus as gateway_event_bus;

use async_trait::async_trait;
use serde_json::Value;

pub use types::EventEmitError;

/// Trait for emitting streaming events
///
/// Implement this trait to receive real-time updates from the agent loop.
/// The default implementation broadcasts events via the Gateway event bus.
#[async_trait]
pub trait EventEmitter: Send + Sync {
    /// Emit a raw stream event
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError>;

    /// Emit a reasoning/thinking update
    async fn emit_reasoning(&self, run_id: &str, content: &str, complete: bool) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq,
                content: content.to_string(),
                is_complete: complete,
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit Reasoning stream event");
        }
    }

    /// Emit tool execution start
    async fn emit_tool_start(&self, run_id: &str, tool_name: &str, tool_id: &str, params: Value) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::ToolStart {
                run_id: run_id.to_string(),
                seq,
                tool_name: tool_name.to_string(),
                tool_id: tool_id.to_string(),
                params,
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit ToolStart stream event");
        }
    }

    /// Emit tool execution progress
    async fn emit_tool_update(&self, run_id: &str, tool_id: &str, progress: &str) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::ToolUpdate {
                run_id: run_id.to_string(),
                seq,
                tool_id: tool_id.to_string(),
                progress: progress.to_string(),
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit ToolUpdate stream event");
        }
    }

    /// Emit tool execution completion
    async fn emit_tool_end(
        &self,
        run_id: &str,
        tool_id: &str,
        result: ToolResult,
        duration_ms: u64,
    ) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::ToolEnd {
                run_id: run_id.to_string(),
                seq,
                tool_id: tool_id.to_string(),
                result,
                duration_ms,
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit ToolEnd stream event");
        }
    }

    /// Emit a structured agent trace event
    async fn emit_agent_trace(&self, run_id: &str, event: crate::harness::trace::LoopTraceEvent) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::AgentTrace {
                run_id: run_id.to_string(),
                seq,
                event: event.into(),
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit AgentTrace stream event");
        }
    }

    /// Emit response text chunk
    async fn emit_response_chunk(
        &self,
        run_id: &str,
        delta: &str,
        full_text: &str,
        chunk_index: u32,
        is_final: bool,
        is_intermediate: bool,
    ) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq,
                delta: delta.to_string(),
                full_text: full_text.to_string(),
                chunk_index,
                is_final,
                is_intermediate,
            })
            .await
        {
            tracing::debug!(run_id, error = %e, "failed to emit ResponseChunk stream event");
        }
    }

    /// Emit run completion
    async fn emit_run_complete(&self, run_id: &str, summary: RunSummary, duration_ms: u64) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq,
                summary,
                total_duration_ms: duration_ms,
            })
            .await
        {
            tracing::warn!(run_id, error = %e, "failed to emit RunComplete stream event");
        }
    }

    /// Emit run error
    async fn emit_run_error(&self, run_id: &str, error: &str, error_code: Option<&str>) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::RunError {
                run_id: run_id.to_string(),
                seq,
                error: error.to_string(),
                error_code: error_code.map(|s| s.to_string()),
            })
            .await
        {
            tracing::warn!(run_id, error = %e, "failed to emit RunError stream event");
        }
    }

    /// Emit a provider-retry status update (transient failure, retrying).
    async fn emit_run_retrying(
        &self,
        run_id: &str,
        provider: &str,
        attempt: u32,
        max_attempts: u32,
        reason: &str,
    ) {
        let seq = self.next_seq();
        if let Err(e) = self
            .emit(StreamEvent::RunRetrying {
                run_id: run_id.to_string(),
                seq,
                provider: provider.to_string(),
                attempt,
                max_attempts,
                reason: reason.to_string(),
            })
            .await
        {
            tracing::warn!(run_id, error = %e, "failed to emit RunRetrying stream event");
        }
    }

    /// Get the next sequence number (must be monotonically increasing)
    fn next_seq(&self) -> u64;
}
