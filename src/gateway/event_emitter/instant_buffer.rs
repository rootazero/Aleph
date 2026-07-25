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
        full_text: content,
        chunk_index: 0,
        is_final: true,
        is_intermediate: false,
    }
}

/// Per-run state behind the instant-mode planner.
///
/// `final_emitted` exists because "buffer empty at `RunComplete`" is ambiguous:
/// it means either *nothing ever streamed* (→ the summary fallback should
/// deliver the answer) or *the `is_final` chunk already flushed it* (→ a
/// fallback would deliver the SAME text a second time — which is exactly what
/// every slash-command fast path and simple-engine run did, since both emit an
/// `is_final` chunk *and* a `RunComplete` carrying `final_response`).
#[derive(Debug, Default)]
pub(super) struct InstantState {
    /// Accumulates streamed response deltas until the final chunk arrives.
    pub(super) buffer: String,
    /// A final chunk has already been emitted for this run.
    pub(super) final_emitted: bool,
}

/// The instant-mode buffering state machine, independent of the sink.
///
/// Mutates `state` (the per-run accumulator) and reports — via
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
/// - `RunComplete` → flush buffered text as a final chunk, then forward the
///   lifecycle event so the channel can finalize. With an empty buffer the
///   sanitized `summary.final_response` is delivered instead — but only when
///   no final chunk was already emitted this run (else it would be the same
///   text twice).
pub(super) fn plan_instant(
    state: &mut InstantState,
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
                    let accumulated = std::mem::take(&mut state.buffer);
                    if accumulated.is_empty() {
                        InstantOutcome::Replace(Vec::new())
                    } else {
                        InstantOutcome::Replace(vec![StreamEvent::ResponseChunk {
                            run_id: run_id.clone(),
                            seq: next_seq(),
                            delta: accumulated.clone(),
                            full_text: accumulated,
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
                state.buffer.push_str(delta);
                InstantOutcome::Buffered
            } else {
                // Final chunk: combine buffered content + this delta.
                let full_content = if state.buffer.is_empty() {
                    delta.clone()
                } else {
                    let buffered = std::mem::take(&mut state.buffer);
                    format!("{buffered}{delta}")
                };
                state.final_emitted = true;
                InstantOutcome::Replace(vec![final_chunk(run_id.clone(), next_seq(), full_content)])
            }
        }

        StreamEvent::RunComplete {
            run_id, summary, ..
        } => {
            let buffered = std::mem::take(&mut state.buffer);
            let flush_text = if buffered.is_empty() {
                if state.final_emitted {
                    // The `is_final` chunk already delivered the full answer —
                    // a summary fallback here re-emitted the SAME text as a
                    // second final chunk (double message on channels, double
                    // print in the CLI) for every slash fast-path / simple
                    // engine run, which emit both. Nothing left to flush.
                    None
                } else {
                    // Fallback: nothing ever streamed (fire-and-forget race) —
                    // deliver the summary's final_response so the channel still
                    // gets the answer. Sanitized through the §4.7 single-source
                    // atom: the raw summary can be pure `<think>` reasoning
                    // (the drain scrubbed the live stream, hence the empty
                    // buffer), which must map to "deliver nothing", not to a
                    // visible reasoning dump.
                    summary
                        .final_response
                        .as_deref()
                        .and_then(crate::gateway::reply_emitter::sanitize_final_response)
                }
            } else {
                Some(buffered)
            };
            // `RunComplete` is the run's terminal event: reset so the planner
            // state is scoped to a run rather than to the emitter that happens
            // to carry it. Without this, `final_emitted` latched forever — a
            // decorator reused for a second run (the type is `pub`, and nothing
            // in the signature says "one per run") would silently suppress that
            // run's summary fallback, and leftover buffered text could splice
            // into the next run's terminal chunk.
            *state = InstantState::default();
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
    /// Per-run planner state (delta accumulator + final-emitted marker).
    state: Mutex<InstantState>,
}

impl<E: EventEmitter> InstantBufferingEmitter<E> {
    /// Wrap `inner` with instant-mode buffering.
    pub fn new(inner: E) -> Self {
        Self {
            inner,
            state: Mutex::new(InstantState::default()),
        }
    }
}

#[async_trait]
impl<E: EventEmitter> EventEmitter for InstantBufferingEmitter<E> {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let outcome = {
            let mut state = self.state.lock().await;
            plan_instant(&mut state, &event, || self.inner.next_seq())
        };
        // (`&mut state` deref-coerces the guard to `&mut InstantState`.)
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
                delta, is_final, ..
            } => {
                assert_eq!(delta, "Hello world");
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
            matches!(&events[0], StreamEvent::ResponseChunk { delta, .. } if delta == "partial")
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

    fn run_complete(final_response: Option<&str>) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: "r1".into(),
            seq: 9,
            summary: crate::gateway::event_emitter::RunSummary {
                final_response: final_response.map(String::from),
                ..Default::default()
            },
            total_duration_ms: 0,
        }
    }

    #[test]
    fn planner_buffers_until_final() {
        let mut buf = InstantState::default();
        assert!(matches!(
            plan_instant(&mut buf, &chunk("a", false), seq_source()),
            InstantOutcome::Buffered
        ));
        assert_eq!(buf.buffer, "a");
        match plan_instant(&mut buf, &chunk("b", true), seq_source()) {
            InstantOutcome::Replace(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { delta, is_final, .. }
                        if delta == "ab" && *is_final)
                );
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        assert!(buf.buffer.is_empty(), "buffer drained after final");
    }

    #[test]
    fn planner_intermediate_marker_flushes_buffer() {
        let mut buf = InstantState {
            buffer: "progress".into(),
            ..Default::default()
        };
        // Empty-delta intermediate = boundary marker → flush as intermediate.
        match plan_instant(&mut buf, &intermediate(""), seq_source()) {
            InstantOutcome::Replace(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { delta, is_intermediate, is_final, .. }
                        if delta == "progress" && *is_intermediate && !*is_final)
                );
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        assert!(buf.buffer.is_empty());
    }

    #[test]
    fn planner_empty_marker_with_empty_buffer_emits_nothing() {
        let mut buf = InstantState::default();
        assert!(matches!(
            plan_instant(&mut buf, &intermediate(""), seq_source()),
            InstantOutcome::Replace(events) if events.is_empty()
        ));
    }

    #[test]
    fn planner_nonempty_intermediate_forwards() {
        let mut buf = InstantState {
            buffer: "buffered".into(),
            ..Default::default()
        };
        assert!(matches!(
            plan_instant(&mut buf, &intermediate("step done"), seq_source()),
            InstantOutcome::Forward
        ));
        // Forwarding must not disturb the running buffer.
        assert_eq!(buf.buffer, "buffered");
    }

    #[test]
    fn planner_run_complete_flushes_buffer_then_forwards() {
        let mut buf = InstantState {
            buffer: "answer".into(),
            ..Default::default()
        };
        match plan_instant(&mut buf, &run_complete(None), seq_source()) {
            InstantOutcome::Prepend(events) => {
                assert_eq!(events.len(), 1);
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { delta, is_final, .. }
                        if delta == "answer" && *is_final)
                );
            }
            other => panic!("expected Prepend, got {other:?}"),
        }
        assert!(buf.buffer.is_empty());
    }

    #[test]
    fn planner_run_complete_falls_back_to_summary() {
        let mut buf = InstantState::default(); // nothing streamed (fire-and-forget race)
        match plan_instant(&mut buf, &run_complete(Some("from summary")), seq_source()) {
            InstantOutcome::Prepend(events) => {
                assert!(
                    matches!(&events[0], StreamEvent::ResponseChunk { delta, .. }
                        if delta == "from summary")
                );
            }
            other => panic!("expected Prepend, got {other:?}"),
        }
    }

    #[test]
    fn planner_no_double_delivery_after_final_chunk() {
        // INSTANT-1: producers that emit BOTH an `is_final` chunk and a
        // `RunComplete` carrying `final_response` (slash fast path, simple
        // engine) must deliver the answer exactly once.
        let mut buf = InstantState::default();
        assert!(matches!(
            plan_instant(&mut buf, &chunk("the answer", true), seq_source()),
            InstantOutcome::Replace(_)
        ));
        assert!(
            matches!(
                plan_instant(&mut buf, &run_complete(Some("the answer")), seq_source()),
                InstantOutcome::Forward
            ),
            "summary fallback must not re-emit the already-flushed final text"
        );
    }

    #[test]
    fn planner_summary_fallback_is_sanitized() {
        // INSTANT-2: a thinking-only terminal turn (live chunks scrubbed →
        // empty buffer, raw summary is pure `<think>`) must deliver nothing,
        // not a visible reasoning dump (§4.7 single-source sanitize).
        let mut buf = InstantState::default();
        assert!(matches!(
            plan_instant(
                &mut buf,
                &run_complete(Some("<think>internal scratch</think>")),
                seq_source()
            ),
            InstantOutcome::Forward
        ));
    }

    #[test]
    fn planner_state_is_scoped_to_a_run_not_to_the_emitter() {
        // INSTANT-3: `final_emitted` used to latch for the life of the
        // planner state. A decorator carrying a SECOND run then swallowed that
        // run's summary fallback (the fallback is the only delivery path when
        // nothing streamed), and stale buffer text could splice into the next
        // run's terminal chunk. `RunComplete` resets.
        let mut buf = InstantState::default();
        // Run 1: a final chunk delivers the answer, RunComplete adds nothing.
        assert!(matches!(
            plan_instant(&mut buf, &chunk("run-1 answer", true), seq_source()),
            InstantOutcome::Replace(_)
        ));
        assert!(matches!(
            plan_instant(&mut buf, &run_complete(Some("run-1 answer")), seq_source()),
            InstantOutcome::Forward
        ));
        // Run 2 on the same planner state: nothing streamed, so the summary
        // fallback MUST fire.
        match plan_instant(&mut buf, &run_complete(Some("run-2 answer")), seq_source()) {
            InstantOutcome::Prepend(events) => assert!(
                matches!(&events[0], StreamEvent::ResponseChunk { delta, .. }
                    if delta == "run-2 answer")
            ),
            other => panic!("second run's summary fallback must fire, got {other:?}"),
        }
    }

    #[test]
    fn planner_silent_run_complete_forwards_only() {
        let mut buf = InstantState::default();
        assert!(matches!(
            plan_instant(&mut buf, &run_complete(None), seq_source()),
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
