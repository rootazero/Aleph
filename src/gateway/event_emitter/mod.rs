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
    OutputMode, RunSummary, StreamEvent, ToolErrorItem, ToolResult, ToolSummaryItem,
    UncertaintyAction,
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

pub use types::EventEmitError;

/// Trait for emitting streaming events
///
/// Implement this trait to receive real-time updates from the agent loop.
/// The default implementation broadcasts events via the Gateway event bus.
#[async_trait]
pub trait EventEmitter: Send + Sync {
    /// Emit a raw stream event
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError>;

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
