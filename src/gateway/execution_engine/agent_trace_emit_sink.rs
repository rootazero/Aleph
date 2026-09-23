//! `AgentTraceEmitSink` — forwards the harness `LoopTraceEvent` stream to the
//! WebSocket event stream as `StreamEvent::AgentTrace`.
//!
//! ## Why this exists
//!
//! The harness emits a structured trace stream (`TurnStarted`, `TextEmitted`,
//! `ToolCallStarted/Completed`, …) via the [`TraceSink`] for persistence and
//! channel progress. The Panel's per-step segmentation and the TUI's step
//! folding consume those events on the wire as `agent_trace` notifications
//! (each carries an `iteration`, the per-step key). The gateway run path
//! drains the *separate* `FlowStreamEvent` stream into `response_chunk` /
//! `tool_start` / `tool_end`; without this sink no `agent_trace` frame would
//! ever be emitted.
//!
//! ## Why it sends on the flow channel
//!
//! `on_trace` does a synchronous `broadcast::Sender::send` of
//! [`FlowStreamEvent::Trace`] onto the run's one flow channel — the channel
//! the harness callback (`on_delta` / `on_reasoning`) publishes on — and the
//! one serial drain (`helpers.rs`) stamps `seq` as it emits. The send happens
//! on the emitting task before `on_trace` returns, so frames emitted by one
//! task keep their emission order in `seq`; nothing assigns a trace frame its
//! `seq` after its neighbours. Frames from different tasks (a tool running on
//! its own task) interleave in the order their sends happen.
//!
//! ## Why it holds a `WeakSender`
//!
//! **This sink never owns the channel.** The gateway drain exits on
//! `RecvError::Closed` whenever the harness returns without a terminal
//! `Complete` (e.g. `runner_impl`'s post-loop "session read" error), and
//! `Closed` only arrives once every STRONG sender is gone. This sink is
//! reachable from holders that outlive the run — a background subagent's
//! `MeteringProvider`s hold the run's chain as their accounting sink inside
//! the child's own `tokio::spawn` (`run_trace_sinks.rs`,
//! `agents/subagent_tool/spawn.rs`). A strong sender here would let such a
//! holder delay the drain's `Closed` for the child's whole lifetime. So the
//! sink keeps a [`broadcast::WeakSender`] and upgrades it per event; once the
//! run's strong senders are gone the upgrade fails and the event is dropped
//! (the run is over — there is no drain left to deliver it to).
//!
//! * It adds **zero** new emit points — it only mirrors events already in
//!   the trace stream, filtered by [`is_step_event`].
//! * It never blocks: `broadcast::send` is synchronous and lossless for the
//!   sender; a receiver that falls behind sees `Lagged`, which the drain
//!   logs (`helpers.rs`) and the client catches at `run_complete` via
//!   `RunSummary.loops`.
//! * A send with zero receivers (the drain already returned) is ignored,
//!   exactly as `BroadcastCallback` ignores it.
//! * It always forwards the original event to the inner sink, so trace
//!   persistence + scratchpad progress are unaffected.
//! * On unattended runs `UnattendedRedactingSink` wraps OUTSIDE this sink
//!   (`run_loop/inner.rs`), so every event is masked before it is published;
//!   `RedactingEmitter` relies on that and passes `AgentTrace` through
//!   (`redacting.rs`), pinned by a test that reads `mask_trace_event`.
//!
//! Only the step-relevant variants are forwarded (see [`is_step_event`]) —
//! the heavy/internal ones (session metrics, worktree/MCP lifecycle) carry no
//! user-facing meaning and would only add wire noise.
//!
//! `ProviderUsage` is an explicit exception to the "internal" rule: it is the
//! sole source of the live prompt-cache reading, and it fires once per LLM
//! call rather than per delta, so the wire-noise argument does not apply.
//! `CacheHealthDegraded` rides along for the same reason: it is the alarm
//! *about* that number, fires once per miss-streak (rising edge), and was a
//! bare log line before — the one signal in this domain that must not stay
//! invisible.

use crate::sync_primitives::Arc;
use tokio::sync::broadcast;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::orchestrator::dispatch::FlowStreamEvent;

/// True for the trace variants the clients consume as `agent_trace`: turn
/// boundaries, authoritative per-step text and reasoning, tool lifecycle, plus the two
/// recovery/watchdog moments that explain *why* the loop changed course —
/// reactive context compaction (problem: context overflow → handled: history
/// compacted → next: retried) and a structural goal-loop veto (problem:
/// checklist incomplete → next: forced continue). Also the three lightweight
/// MoA fan-out moments (advisor answer, aggregator hand-off, advisor spend) —
/// `MoaTurnTrace` is deliberately excluded: it carries the full advisor I/O
/// payload and is persisted-only, never wire.
///
/// And `ProviderUsage`, which is what the TUI's `cache N%` cell is built from
/// (`interfaces/tui/.../app/trace.rs` → `AppState.cache_stat` →
/// `widgets/status_bar.rs`). It was previously dropped here as "internal",
/// which left that cell — the product's only *live* prompt-cache indicator —
/// unable to fire during a run: it could only appear when a user manually
/// replayed a persisted trace, after the fact. A broken prefix is silent by
/// nature (the symptom is the bill), so this is the one number that has to
/// reach a live surface. Volume is one event per LLM call, not per delta.
///
/// Everything else is dropped — it carries no user-facing meaning.
pub(crate) const fn is_step_event(event: &LoopTraceEvent) -> bool {
    matches!(
        event,
        LoopTraceEvent::TurnStarted { .. }
            | LoopTraceEvent::TextEmitted { .. }
            | LoopTraceEvent::ReasoningEmitted { .. }
            | LoopTraceEvent::ToolCallStarted { .. }
            | LoopTraceEvent::ToolCallCompleted { .. }
            | LoopTraceEvent::ReactiveCompactionAttempted { .. }
            | LoopTraceEvent::VerifierVeto { .. }
            | LoopTraceEvent::MoaAdvisor { .. }
            | LoopTraceEvent::MoaAggregating { .. }
            | LoopTraceEvent::MoaAdvisorSpend { .. }
            | LoopTraceEvent::ProviderUsage { .. }
            | LoopTraceEvent::CacheHealthDegraded { .. }
    )
}

/// Decorator over a parent [`TraceSink`] that mirrors step-relevant trace
/// events onto the run's flow channel, while always forwarding the original
/// event to the inner sink.
///
/// Holds only a [`broadcast::WeakSender`]: this sink never owns the channel,
/// so a holder that outlives the run cannot delay the drain's `Closed`.
pub struct AgentTraceEmitSink {
    inner: Arc<dyn TraceSink>,
    tx: broadcast::WeakSender<FlowStreamEvent>,
}

impl AgentTraceEmitSink {
    /// Wrap `inner`. `tx` must be the SAME channel the run's harness callback
    /// publishes on (`FlowRequest.event_tx`, created by
    /// `orchestrator::flow_event_channel`) — a second channel would put the
    /// ordering race straight back.
    ///
    /// Takes the sender by reference and keeps only its weak half: this sink
    /// never owns the channel, and a holder that outlives the run (a
    /// background subagent's metering, which holds the run's chain as its
    /// accounting sink) cannot delay the drain's `Closed`.
    pub fn new(inner: Arc<dyn TraceSink>, tx: &broadcast::Sender<FlowStreamEvent>) -> Self {
        Self {
            inner,
            tx: tx.downgrade(),
        }
    }
}

impl TraceSink for AgentTraceEmitSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if is_step_event(event) {
            // `upgrade` fails once the run's strong senders are gone — the run
            // is over and its drain has (or is about to have) returned, so the
            // mirror has nowhere to go. The only `send` error is "zero
            // receivers" (the drain is gone). Neither may abort the harness
            // loop — same rule as `BroadcastCallback`.
            if let Some(tx) = self.tx.upgrade() {
                let _ = tx.send(FlowStreamEvent::Trace(event.clone().into()));
            }
        }
        self.inner.on_trace(event);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};
    use crate::harness::trace_sink::NoopTraceSink;
    use crate::orchestrator::flow_event_channel;

    #[test]
    fn forwards_turn_and_text_and_tool_events() {
        assert!(is_step_event(&LoopTraceEvent::TurnStarted { iteration: 1 }));
        assert!(is_step_event(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "hi".into(),
        }));
        assert!(is_step_event(&LoopTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "why".into(),
        }));
        assert!(is_step_event(&LoopTraceEvent::TurnStarted { iteration: 2 }));
    }

    #[test]
    fn forwards_recovery_and_watchdog_events() {
        // The two "why did the loop change course" moments must reach the wire.
        assert!(is_step_event(
            &LoopTraceEvent::ReactiveCompactionAttempted {
                token_gap: Some(1200),
                succeeded: true,
            }
        ));
        assert!(is_step_event(&LoopTraceEvent::VerifierVeto {
            iteration: 3,
            reason: "- [ ] ship auth".into(),
        }));
    }

    #[test]
    fn drops_non_step_events() {
        // Session metrics are not Panel-relevant — must not hit the wire.
        let session_completed = LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 2,
            tool_calls_made: 1,
            total_tokens: 10,
            hit_limit: false,
            final_text: None,
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        };
        assert!(!is_step_event(&session_completed));
        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), &tx);
        sink.on_trace(&session_completed);
        assert!(
            rx.try_recv().is_err(),
            "a non-step event must not be published on the flow channel"
        );
    }

    /// The mirror publishes SYNCHRONOUSLY onto the run's flow channel — no
    /// spawned task, no `.await`: this `try_recv`, made with no runtime at
    /// all, must already see the frame. Red if `on_trace` hands the send to
    /// another task. The test's own strong `tx` stays alive, so the sink's
    /// weak half upgrades.
    #[test]
    fn on_trace_publishes_onto_the_flow_channel_before_returning() {
        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), &tx);

        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 2 });

        match rx.try_recv() {
            Ok(FlowStreamEvent::Trace(aleph_protocol::AgentTraceEvent::TurnStarted {
                iteration,
            })) => assert_eq!(iteration, 2),
            other => panic!("expected the turn boundary on the flow channel, got {other:?}"),
        }
        drop(tx);
    }

    /// `ProviderUsage` must REACH the wire, not merely satisfy the predicate —
    /// the TUI's live `cache N%` cell is built from it. Drained through the
    /// real `emit_flow_event` so the assertion is on the delivered frame.
    #[tokio::test]
    async fn provider_usage_reaches_the_emitter() {
        use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
        use crate::gateway::execution_engine::event_drain::{emit_flow_event, DrainState};

        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), &tx);
        sink.on_trace(&LoopTraceEvent::ProviderUsage {
            agent_id: "main".into(),
            input_tokens: 120,
            output_tokens: 30,
            cache_read_tokens: Some(4_000),
            cache_creation_tokens: Some(0),
            thinking_tokens: None,
        });

        let inner = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = inner.clone();
        let state = Arc::new(tokio::sync::Mutex::new(DrainState::default()));
        while let Ok(ev) = rx.try_recv() {
            emit_flow_event(ev, &emitter, "run-cache", &state)
                .await
                .expect("drain ok");
        }
        assert!(
            inner
                .events()
                .await
                .iter()
                .any(|e| matches!(e, StreamEvent::AgentTrace { .. })),
            "ProviderUsage must be mirrored to the wire — the live cache cell reads it"
        );
        drop(tx);
    }

    /// The sink never owns the channel: once the last strong sender drops, a
    /// receiver sees `Closed` even while the sink itself is still alive (the
    /// shape of a background subagent's metering holding the run's chain past
    /// the run),
    /// and a later `on_trace` is a silent drop, not a panic. Red if the sink
    /// stores a strong `broadcast::Sender`.
    #[tokio::test]
    async fn a_live_sink_does_not_keep_the_channel_open() {
        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), &tx);
        drop(tx);

        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
            Ok(Err(broadcast::error::RecvError::Closed)) => {}
            other => panic!("the live sink kept the channel open: {other:?}"),
        }
        drop(sink);
    }
}
