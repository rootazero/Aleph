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
//! The buffering state machine is **single-sourced** in [`plan_instant`]; both
//! this decorator and the gateway's own `GatewayEventEmitter` route through it,
//! differing only in the sink (an inner `EventEmitter` here, the event bus
//! there). Keeping one planner is what stops the two from drifting.

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::types::{EventEmitError, StreamEvent};
use super::EventEmitter;

/// Sink-agnostic decision for one event fed to the instant-mode buffer.
///
/// The planner only *borrows* the event, so the caller keeps ownership and
/// reuses its own forwarding path for [`Forward`](InstantOutcome::Forward) /
/// [`Prepend`](InstantOutcome::Prepend) — no clone of the passthrough event.
#[derive(Debug)]
pub(super) enum InstantOutcome {
    /// Delta was absorbed into the buffer; emit nothing for the original event.
    Buffered,
    /// Drop the original event; emit exactly these (already-sequenced) events.
    Replace(Vec<StreamEvent>),
    /// Emit these events first, then forward the original event unchanged.
    Prepend(Vec<StreamEvent>),
    /// Forward the original event unchanged (no buffering interaction).
    Forward,
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

/// The instant-mode buffering state machine, independent of the sink.
///
/// Mutates `buffer` (the per-run accumulator) and reports — via
/// [`InstantOutcome`] — what the caller should emit. `next_seq` is pulled only
/// when a replacement/flush chunk is synthesized, so the wire keeps a single
/// monotonic sequence regardless of which path fired.
///
/// Behavior (response *text* is coalesced; status/lifecycle events pass through):
/// - intermediate boundary marker (empty delta) → flush buffer as a standalone
///   intermediate chunk, or nothing if empty;
/// - non-empty intermediate chunk → forwarded immediately, standalone;
/// - normal streaming delta → buffered, nothing emitted;
/// - final chunk → buffer + delta emitted as one final chunk;
/// - `RunComplete` → flush buffered text (or fall back to
///   `summary.final_response`) as a final chunk, then forward the lifecycle
///   event so the channel can finalize.
pub(super) fn plan_instant(
    buffer: &mut String,
    event: &StreamEvent,
    mut next_seq: impl FnMut() -> u64,
) -> InstantOutcome {
    match event {
        StreamEvent::ResponseChunk {
            delta,
            is_final,
            is_intermediate,
            run_id,
            ..
        } => {
            if *is_intermediate {
                if delta.is_empty() {
                    // Boundary marker: flush whatever has accumulated so far.
                    let accumulated = std::mem::take(buffer);
                    if accumulated.is_empty() {
                        InstantOutcome::Replace(Vec::new())
                    } else {
                        InstantOutcome::Replace(vec![StreamEvent::ResponseChunk {
                            run_id: run_id.clone(),
                            seq: next_seq(),
                            delta: accumulated.clone(),
                            full_text: accumulated.clone(),
                            content: accumulated,
                            chunk_index: 0,
                            is_final: false,
                            is_intermediate: true,
                        }])
                    }
                } else {
                    // Non-empty intermediate: surface immediately, untouched.
                    InstantOutcome::Forward
                }
            } else if !*is_final {
                buffer.push_str(delta);
                InstantOutcome::Buffered
            } else {
                // Final chunk: combine buffered content + this delta.
                let full_content = if buffer.is_empty() {
                    delta.clone()
                } else {
                    let buffered = std::mem::take(buffer);
                    format!("{buffered}{delta}")
                };
                InstantOutcome::Replace(vec![final_chunk(
                    run_id.clone(),
                    next_seq(),
                    full_content,
                )])
            }
        }

        StreamEvent::RunComplete {
            run_id, summary, ..
        } => {
            let buffered = std::mem::take(buffer);
            let flush_text = if buffered.is_empty() {
                // Fallback: buffer empty (e.g. fire-and-forget race) — use the
                // summary's final_response so the channel still gets the answer.
                summary
                    .final_response
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .cloned()
            } else {
                Some(buffered)
            };
            match flush_text {
                Some(text) => {
                    InstantOutcome::Prepend(vec![final_chunk(run_id.clone(), next_seq(), text)])
                }
                None => InstantOutcome::Forward,
            }
        }

        // Tool / reasoning / lifecycle events are never buffered.
        _ => InstantOutcome::Forward,
    }
}

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
}

#[async_trait]
impl<E: EventEmitter> EventEmitter for InstantBufferingEmitter<E> {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let outcome = {
            let mut buffer = self.buffer.lock().await;
            plan_instant(&mut buffer, &event, || self.inner.next_seq())
        };
        // (`&mut buffer` deref-coerces the guard to `&mut String`.)
        match outcome {
            InstantOutcome::Buffered => Ok(()),
            InstantOutcome::Replace(events) => {
                for e in events {
                    self.inner.emit(e).await?;
                }
                Ok(())
            }
            InstantOutcome::Prepend(events) => {
                for e in events {
                    self.inner.emit(e).await?;
                }
                self.inner.emit(event).await
            }
            InstantOutcome::Forward => self.inner.emit(event).await,
        }
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

    // ── planner-level tests (sink-agnostic state machine) ──────────────────

    fn intermediate(delta: &str) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id: "r1".into(),
            seq: 0,
            delta: delta.into(),
            full_text: delta.into(),
            content: delta.into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: true,
        }
    }

    /// A counter usable as the `next_seq` closure.
    fn seq_source() -> impl FnMut() -> u64 {
        let mut n = 100;
        move || {
            n += 1;
            n
        }
    }

    #[test]
    fn planner_buffers_until_final() {
        let mut buf = String::new();
        assert!(matches!(
            plan_instant(&mut buf, &chunk("a", false), seq_source()),
            InstantOutcome::Buffered
        ));
        assert_eq!(buf, "a");
        match plan_instant(&mut buf, &chunk("b", true), seq_source()) {
            InstantOutcome::Replace(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { content, is_final, .. }
                        if content == "ab" && *is_final)
                );
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        assert!(buf.is_empty(), "buffer drained after final");
    }

    #[test]
    fn planner_intermediate_marker_flushes_buffer() {
        let mut buf = String::from("progress");
        // Empty-delta intermediate = boundary marker → flush as intermediate.
        match plan_instant(&mut buf, &intermediate(""), seq_source()) {
            InstantOutcome::Replace(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { content, is_intermediate, is_final, .. }
                        if content == "progress" && *is_intermediate && !*is_final)
                );
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn planner_empty_marker_with_empty_buffer_emits_nothing() {
        let mut buf = String::new();
        assert!(matches!(
            plan_instant(&mut buf, &intermediate(""), seq_source()),
            InstantOutcome::Replace(events) if events.is_empty()
        ));
    }

    #[test]
    fn planner_nonempty_intermediate_forwards() {
        let mut buf = String::from("buffered");
        assert!(matches!(
            plan_instant(&mut buf, &intermediate("step done"), seq_source()),
            InstantOutcome::Forward
        ));
        // Forwarding must not disturb the running buffer.
        assert_eq!(buf, "buffered");
    }

    #[test]
    fn planner_run_complete_flushes_buffer_then_forwards() {
        let mut buf = String::from("answer");
        let rc = StreamEvent::RunComplete {
            run_id: "r1".into(),
            seq: 9,
            summary: crate::gateway::event_emitter::RunSummary::default(),
            total_duration_ms: 0,
        };
        match plan_instant(&mut buf, &rc, seq_source()) {
            InstantOutcome::Prepend(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { content, is_final, .. }
                        if content == "answer" && *is_final)
                );
            }
            other => panic!("expected Prepend, got {other:?}"),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn planner_run_complete_falls_back_to_summary() {
        let mut buf = String::new(); // buffer empty (fire-and-forget race)
        let rc = StreamEvent::RunComplete {
            run_id: "r1".into(),
            seq: 9,
            summary: crate::gateway::event_emitter::RunSummary {
                final_response: Some("from summary".into()),
                ..Default::default()
            },
            total_duration_ms: 0,
        };
        match plan_instant(&mut buf, &rc, seq_source()) {
            InstantOutcome::Prepend(events) => {
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { content, .. }
                        if content == "from summary")
                );
            }
            other => panic!("expected Prepend, got {other:?}"),
        }
    }

    #[test]
    fn planner_silent_run_complete_forwards_only() {
        let mut buf = String::new();
        let rc = StreamEvent::RunComplete {
            run_id: "r1".into(),
            seq: 9,
            summary: crate::gateway::event_emitter::RunSummary::default(),
            total_duration_ms: 0,
        };
        assert!(matches!(
            plan_instant(&mut buf, &rc, seq_source()),
            InstantOutcome::Forward
        ));
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
