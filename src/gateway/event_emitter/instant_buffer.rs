//! Instant-mode buffering decorator
//!
//! A channel-agnostic [`EventEmitter`] wrapper that enforces the global
//! `output_mode = "instant"` switch on top of *any* inner emitter — including
//! channel-specific emitters that stream independently of the gateway's own
//! [`GatewayEventEmitter`](super::impls::GatewayEventEmitter).
//!
//! # Why a decorator?
//!
//! `output_mode` ("typewriter" vs "instant") is a single global switch read
//! fresh per-run by every channel path. The panel's `GatewayEventEmitter`
//! honors it by buffering chunks before they hit the event bus, and the
//! chat-channel `ReplyEmitter` honors it via `stream_enabled`. But emitters
//! that take over the wire themselves — notably Telegram's orchestrated
//! `TelegramEventEmitter` — bypass `ReplyEmitter` entirely and would stream
//! regardless of the switch. Wrapping such an emitter with this decorator
//! restores "one switch → all channels synchronized": in instant mode the
//! decorator buffers `ResponseChunk` deltas and forwards only the final
//! combined chunk, so the underlying channel sends a single message instead of
//! a progressive typewriter stream.
//!
//! The buffering state machine mirrors `GatewayEventEmitter`'s proven logic;
//! the difference is the sink (an inner `EventEmitter` rather than the event
//! bus), which is exactly what makes it reusable across channels.

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::types::{EventEmitError, StreamEvent};
use super::EventEmitter;

/// Wraps an inner [`EventEmitter`] and applies instant-mode buffering.
///
/// Non-`ResponseChunk` events (tool/reasoning/lifecycle) always pass straight
/// through, so status indicators are unaffected — only the response *text* is
/// buffered into a single final message.
pub struct InstantBufferingEmitter<E: EventEmitter> {
    inner: E,
    /// Accumulates streamed response deltas until the final chunk arrives.
    buffer: Mutex<String>,
}

impl<E: EventEmitter> InstantBufferingEmitter<E> {
    /// Wrap `inner` with instant-mode buffering.
    pub fn new(inner: E) -> Self {
        Self {
            inner,
            buffer: Mutex::new(String::new()),
        }
    }

    /// Build a final, non-intermediate `ResponseChunk` carrying `content`.
    fn final_chunk(run_id: String, seq: u64, content: String) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id,
            seq,
            delta: content.clone(),
            full_text: content.clone(),
            content,
            chunk_index: 0,
            is_final: true,
            is_intermediate: false,
        }
    }
}

#[async_trait]
impl<E: EventEmitter> EventEmitter for InstantBufferingEmitter<E> {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        if let StreamEvent::ResponseChunk {
            ref delta,
            is_final,
            is_intermediate,
            ref run_id,
            seq,
            ..
        } = event
        {
            if is_intermediate {
                if delta.is_empty() {
                    // Intermediate boundary marker: flush the accumulated buffer
                    // as a standalone intermediate message, then clear it.
                    let accumulated = {
                        let mut buffer = self.buffer.lock().await;
                        std::mem::take(&mut *buffer)
                    };
                    if !accumulated.is_empty() {
                        let frame = StreamEvent::ResponseChunk {
                            run_id: run_id.clone(),
                            seq,
                            delta: accumulated.clone(),
                            full_text: accumulated.clone(),
                            content: accumulated,
                            chunk_index: 0,
                            is_final: false,
                            is_intermediate: true,
                        };
                        self.inner.emit(frame).await?;
                    }
                } else {
                    // Non-empty intermediate: emit immediately, standalone.
                    self.inner.emit(event).await?;
                }
                return Ok(());
            }

            if !is_final {
                // Buffer the chunk delta; swallow until the final chunk.
                self.buffer.lock().await.push_str(delta);
                return Ok(());
            }

            // Final chunk: combine buffered content + this delta, emit once.
            let full_content = {
                let mut buffer = self.buffer.lock().await;
                if buffer.is_empty() {
                    delta.clone()
                } else {
                    let buffered = std::mem::take(&mut *buffer);
                    format!("{}{}", buffered, delta)
                }
            };
            self.inner
                .emit(Self::final_chunk(run_id.clone(), seq, full_content))
                .await?;
            return Ok(());
        }

        if let StreamEvent::RunComplete {
            ref run_id,
            ref summary,
            ..
        } = event
        {
            // Flush any buffered content as the final response before the
            // lifecycle event reaches the channel.
            let buffered = {
                let mut buffer = self.buffer.lock().await;
                std::mem::take(&mut *buffer)
            };
            if !buffered.is_empty() {
                self.inner
                    .emit(Self::final_chunk(run_id.clone(), self.next_seq(), buffered))
                    .await?;
            } else if let Some(final_response) = summary.final_response.as_ref() {
                // Fallback: buffer empty (e.g. fire-and-forget race) — use the
                // summary's final_response so the channel still gets the answer.
                if !final_response.is_empty() {
                    self.inner
                        .emit(Self::final_chunk(
                            run_id.clone(),
                            self.next_seq(),
                            final_response.clone(),
                        ))
                        .await?;
                }
            }
            // Forward the RunComplete itself so the channel can finalize.
            return self.inner.emit(event).await;
        }

        // Everything else (tool/reasoning/lifecycle) passes through untouched.
        self.inner.emit(event).await
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::CollectingEventEmitter;
    use crate::sync_primitives::Arc;

    fn chunk(delta: &str, is_final: bool) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id: "r1".into(),
            seq: 0,
            delta: delta.into(),
            full_text: delta.into(),
            content: delta.into(),
            chunk_index: 0,
            is_final,
            is_intermediate: false,
        }
    }

    /// In instant mode, streamed deltas are buffered and delivered as a single
    /// final chunk — never as a progressive stream.
    #[tokio::test]
    async fn buffers_deltas_into_single_final_chunk() {
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter = InstantBufferingEmitter::new(DelegateToArc(collector.clone()));

        emitter.emit(chunk("Hel", false)).await.unwrap();
        emitter.emit(chunk("lo ", false)).await.unwrap();
        emitter.emit(chunk("world", true)).await.unwrap();

        let events = collector.events().await;
        // Only the final combined chunk should have been forwarded.
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                assert_eq!(content, "Hello world");
                assert!(is_final);
            }
            other => panic!("expected final ResponseChunk, got {other:?}"),
        }
    }

    /// Non-ResponseChunk events pass through unchanged even in instant mode.
    #[tokio::test]
    async fn passes_through_non_response_events() {
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter = InstantBufferingEmitter::new(DelegateToArc(collector.clone()));

        emitter
            .emit(StreamEvent::ToolStart {
                run_id: "r1".into(),
                seq: 1,
                tool_name: "bash".into(),
                tool_id: "t1".into(),
                params: serde_json::Value::Null,
            })
            .await
            .unwrap();

        let events = collector.events().await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::ToolStart { .. }));
    }

    /// A RunComplete flushes buffered text (when no final chunk arrived) and
    /// then forwards the lifecycle event.
    #[tokio::test]
    async fn flushes_buffer_on_run_complete() {
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter = InstantBufferingEmitter::new(DelegateToArc(collector.clone()));

        emitter.emit(chunk("partial", false)).await.unwrap();
        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "r1".into(),
                seq: 9,
                summary: crate::gateway::event_emitter::RunSummary::default(),
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let events = collector.events().await;
        // Flushed chunk + the RunComplete itself.
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], StreamEvent::ResponseChunk { content, .. } if content == "partial")
        );
        assert!(matches!(events[1], StreamEvent::RunComplete { .. }));
    }

    /// Test helper: an `EventEmitter` that delegates to a shared
    /// `Arc<CollectingEventEmitter>` so tests can inspect forwarded events.
    struct DelegateToArc(Arc<CollectingEventEmitter>);

    #[async_trait]
    impl EventEmitter for DelegateToArc {
        async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
            self.0.emit(event).await
        }
        fn next_seq(&self) -> u64 {
            self.0.next_seq()
        }
    }
}
