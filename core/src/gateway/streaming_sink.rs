//! Real-time streaming bridge: ProviderDelta → EventEmitter
//!
//! Created per-run by ExecutionEngine, injected into AgentLoop via with_delta_sink().
//! Buffers text deltas within a 100ms throttle window for smooth delivery.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::gateway::event_emitter::EventEmitter;
use crate::providers::delta::{DeltaSink, ProviderDelta};
use crate::sync_primitives::Arc;

/// Throttle window in milliseconds — buffers text deltas within this window
const THROTTLE_MS: u64 = 100;

/// Bridges ProviderDelta events to the Gateway EventEmitter for real-time text streaming.
///
/// Created per-run by ExecutionEngine and injected into AgentLoop via with_delta_sink().
/// Buffers text deltas within a 100ms throttle window to avoid flooding WebSocket
/// connections with tiny single-token messages.
///
/// On `Done`, flushes any remaining buffer and emits a final marker chunk with
/// `is_final = true` and empty content, signalling the client that the stream is complete.
pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
    /// Shared with StreamCallback to coordinate text deduplication.
    /// Set to true when at least one TextDelta has been emitted.
    /// StreamCallback reads and resets via swap(false).
    has_emitted_text: Arc<AtomicBool>,
    buffer: Mutex<String>,
    last_emit: Mutex<Instant>,
}

impl StreamingDeltaSink {
    /// Create a new StreamingDeltaSink.
    ///
    /// # Arguments
    /// - `run_id` — the run identifier, forwarded to all emitted events
    /// - `emitter` — the EventEmitter to broadcast response chunks to
    /// - `has_emitted_text` — shared flag; set to `true` after any TextDelta is forwarded
    pub fn new(
        run_id: String,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
        has_emitted_text: Arc<AtomicBool>,
    ) -> Self {
        Self {
            run_id,
            emitter,
            chunk_index: AtomicU32::new(0),
            has_emitted_text,
            buffer: Mutex::new(String::new()),
            last_emit: Mutex::new(Instant::now()),
        }
    }
}

#[async_trait]
impl DeltaSink for StreamingDeltaSink {
    async fn on_delta(&self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(text) => {
                let mut buf = self.buffer.lock().await;
                buf.push_str(text);

                let last = self.last_emit.lock().await;
                let elapsed = last.elapsed().as_millis() as u64;
                drop(last);

                if elapsed >= THROTTLE_MS {
                    let content = std::mem::take(&mut *buf);
                    drop(buf);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter.emit_response_chunk(&self.run_id, &content, idx, false).await;
                    self.has_emitted_text.store(true, Ordering::Release);
                    *self.last_emit.lock().await = Instant::now();
                }
            }
            ProviderDelta::Done(_) => {
                // Flush any remaining buffered text
                let mut buf = self.buffer.lock().await;
                let remaining = std::mem::take(&mut *buf);
                drop(buf);

                if !remaining.is_empty() {
                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter.emit_response_chunk(&self.run_id, &remaining, idx, false).await;
                    self.has_emitted_text.store(true, Ordering::Release);
                }

                // Emit final marker — empty content + is_final = true
                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                self.emitter.emit_response_chunk(&self.run_id, "", idx, true).await;
            }
            // ThinkingDelta, ToolCall*, Usage, Error — not handled in Phase 2
            _ => {}
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
    use crate::providers::adapter::StopReason;
    use std::sync::atomic::AtomicBool;

    /// Helper: create a sink backed by a CollectingEventEmitter
    fn make_sink(
        run_id: &str,
        has_emitted_text: Arc<AtomicBool>,
    ) -> (StreamingDeltaSink, Arc<CollectingEventEmitter>) {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let sink = StreamingDeltaSink::new(
            run_id.to_string(),
            emitter.clone() as Arc<dyn EventEmitter + Send + Sync>,
            has_emitted_text,
        );
        (sink, emitter)
    }

    /// Send TextDelta followed by Done; expect buffer flushed and final marker emitted.
    #[tokio::test]
    async fn test_done_flushes_buffer_and_emits_final() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-1", flag.clone());

        // TextDelta — will be buffered (elapsed < 100ms)
        sink.on_delta(&ProviderDelta::TextDelta("hello world".to_string())).await;

        // Done — should flush buffer + emit final marker
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;

        let events = emitter.events().await;

        // Must have at least two events: the flushed text chunk and the final marker
        assert!(events.len() >= 2, "Expected at least 2 events, got {}", events.len());

        // The second-to-last event should contain the text content
        let text_event = events.iter().find(|e| {
            matches!(e, StreamEvent::ResponseChunk { content, is_final, .. }
                if !content.is_empty() && !*is_final)
        });
        assert!(text_event.is_some(), "Expected a non-final chunk with text content");

        // The last event must be the final marker
        let last = events.last().unwrap();
        assert!(
            matches!(last, StreamEvent::ResponseChunk { content, is_final, .. }
                if *is_final && content.is_empty()),
            "Last event must be final empty marker, got: {:?}",
            last
        );
    }

    /// Verify has_emitted_text flag is set after a TextDelta emission.
    #[tokio::test]
    async fn test_has_emitted_text_flag_set() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, _emitter) = make_sink("run-2", flag.clone());

        // Not set before any deltas
        assert!(!flag.load(Ordering::Acquire));

        // Done with buffered text → flush triggers flag
        sink.on_delta(&ProviderDelta::TextDelta("some text".to_string())).await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;

        assert!(flag.load(Ordering::Acquire), "has_emitted_text flag should be true after text emission");
    }

    /// Non-text deltas (ThinkingDelta, ToolCallStart) should produce no events.
    #[tokio::test]
    async fn test_non_text_deltas_ignored() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-3", flag.clone());

        sink.on_delta(&ProviderDelta::ThinkingDelta("hmm".to_string())).await;
        sink.on_delta(&ProviderDelta::ToolCallStart {
            id: "tc1".to_string(),
            name: "search".to_string(),
        }).await;

        let events = emitter.events().await;
        assert!(events.is_empty(), "Non-text deltas should not produce any events");
        assert!(!flag.load(Ordering::Acquire), "Flag should remain unset");
    }

    /// Done with no prior TextDelta should only emit the final marker (no text chunk).
    #[tokio::test]
    async fn test_empty_buffer_done_only_emits_final() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-4", flag.clone());

        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 1, "Should emit exactly one event (the final marker)");

        let only = &events[0];
        assert!(
            matches!(only, StreamEvent::ResponseChunk { content, is_final, .. }
                if *is_final && content.is_empty()),
            "The single event should be the final empty marker, got: {:?}",
            only
        );

        // Flag should remain unset since no text was emitted
        assert!(!flag.load(Ordering::Acquire), "Flag should remain unset when no text was emitted");
    }
}
