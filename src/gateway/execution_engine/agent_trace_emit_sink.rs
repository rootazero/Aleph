//! `AgentTraceEmitSink` — forwards the harness `LoopTraceEvent` stream to the
//! WebSocket event stream as `StreamEvent::AgentTrace`.
//!
//! ## Why this exists
//!
//! The harness emits a structured trace stream (`TurnStarted`, `TextEmitted`,
//! `ToolCallStarted/Completed`, …) via the [`TraceSink`] for persistence and
//! channel progress. The `WebChat` Panel's per-step segmentation + workspace
//! timeline are built to consume those events on the wire as `agent_trace`
//! notifications (each carries an `iteration`, the per-step key). But the
//! gateway run path drains the *separate* `FlowStreamEvent` stream into
//! `response_chunk` / legacy `tool_start` / `tool_end` only — it never emitted
//! `StreamEvent::AgentTrace`. So the Panel's `agent_trace` branch never fired:
//! the chat never segmented and the workspace pane stayed blank.
//!
//! This sink closes that gap the same way [`super::ScratchpadProgressSink`]
//! does — a thin decorator that mirrors trace events to the user surface:
//!
//! * It adds **zero** new `LoopTraceEvent` variants and **zero** harness emit
//!   points — it only reads events already flowing through the trace stream.
//! * It never blocks: `on_trace` does a non-blocking `mpsc` send; the async
//!   `emit` drains on a single spawned task, so per-step ordering (turn before
//!   its tools/text) is preserved.
//! * It always forwards the original event to the inner sink, so trace
//!   persistence + scratchpad progress are unaffected.
//!
//! Only the step-relevant variants are forwarded (see [`is_step_event`]) —
//! the heavy/internal ones (session metrics, provider usage, worktree/MCP
//! lifecycle) carry no Panel meaning and would only add wire noise.

use crate::sync_primitives::Arc;
use tokio::sync::mpsc;

use crate::gateway::event_emitter::EventEmitter;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// True for the trace variants the `WebChat` Panel consumes as `agent_trace`
/// (`views/chat/events.rs`): turn boundaries, authoritative per-step text, and
/// tool lifecycle, plus the two recovery/watchdog moments that explain *why*
/// the loop changed course — reactive context compaction (problem: context
/// overflow → handled: history compacted → next: retried) and a structural
/// goal-loop veto (problem: checklist incomplete → next: forced continue).
/// Also the three lightweight MoA fan-out moments (advisor answer, aggregator
/// hand-off, advisor spend) — `MoaTurnTrace` is deliberately excluded: it
/// carries the full advisor I/O payload and is persisted-only, never wire.
/// Everything else is dropped — it carries no user-facing meaning.
pub(crate) const fn is_step_event(event: &LoopTraceEvent) -> bool {
    matches!(
        event,
        LoopTraceEvent::TurnStarted { .. }
            | LoopTraceEvent::TextEmitted { .. }
            | LoopTraceEvent::ToolCallStarted { .. }
            | LoopTraceEvent::ToolCallCompleted { .. }
            | LoopTraceEvent::ReactiveCompactionAttempted { .. }
            | LoopTraceEvent::VerifierVeto { .. }
            | LoopTraceEvent::MoaAdvisor { .. }
            | LoopTraceEvent::MoaAggregating { .. }
            | LoopTraceEvent::MoaAdvisorSpend { .. }
    )
}

/// Decorator over a parent [`TraceSink`] that mirrors step-relevant trace
/// events to the run's WebSocket stream, while always forwarding the original
/// event to the inner sink.
pub struct AgentTraceEmitSink {
    inner: Arc<dyn TraceSink>,
    tx: mpsc::Sender<LoopTraceEvent>,
}

impl AgentTraceEmitSink {
    /// Wrap `inner`, spawning a background drain task that emits each
    /// step-relevant event as `StreamEvent::AgentTrace` for `run_id` via
    /// `emitter`. The task ends when this sink (and its sender) drops.
    pub fn new(inner: Arc<dyn TraceSink>, emitter: Arc<dyn EventEmitter>, run_id: String) -> Self {
        // Bounded queue: trace events fire per agent-loop step, faster than
        // the WebSocket consumer may drain. Overflow drops the event
        // (best-effort mirror), same as a closed receiver.
        let (tx, mut rx) = mpsc::channel::<LoopTraceEvent>(256);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // `emit_agent_trace` assigns the next seq and pushes
                // `StreamEvent::AgentTrace { run_id, seq, event }`.
                emitter.emit_agent_trace(&run_id, event).await;
            }
        });
        Self { inner, tx }
    }
}

impl TraceSink for AgentTraceEmitSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if is_step_event(event) {
            // Non-blocking; drop when the queue is full (slow consumer) or the
            // receiver is closed (drain task gone) — the mirror is best-effort.
            let _ = self.tx.try_send(event.clone());
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
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};

    #[test]
    fn forwards_turn_and_text_and_tool_events() {
        assert!(is_step_event(&LoopTraceEvent::TurnStarted { iteration: 1 }));
        assert!(is_step_event(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "hi".into(),
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
    }
}
