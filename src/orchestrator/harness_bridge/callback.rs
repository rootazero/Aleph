//! Broadcast callback adapter that fans `HarnessCallback` lifecycle events
//! onto the orchestrator's `FlowStreamEvent` broadcast channel — plus the one
//! durable receipt those live-only frames cannot leave behind
//! ([`record_input_block`]). The latch and the receipt live together because
//! the latch has no other consumer.

use crate::harness::callback::HarnessCallback;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
use crate::session::events::{ErrorKind, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::{SessionId, SessionService};
use tokio::sync::broadcast;

/// Adapter that fans `HarnessCallback` lifecycle events onto the
/// orchestrator's `FlowStreamEvent` broadcast channel.
///
/// * `on_delta(text)` → `FlowStreamEvent::Delta(text)`
/// * `on_reasoning(text)` → `FlowStreamEvent::Reasoning(text)`
/// * `on_tool_call_start(id, name, args)` → `FlowStreamEvent::ToolCallStart { id, name, args }`
/// * `on_tool_call_done(id, result, error, duration_ms)` → `FlowStreamEvent::ToolCallDone { id, result, error, duration_ms }`
/// * `on_context_usage(tokens, total)` → `FlowStreamEvent::ContextGauge
///   { context_tokens, context_window, total_tokens }` (window pre-resolved
///   at construction; suppressed when either side is 0)
/// * `on_safety_block(reason)` → `FlowStreamEvent::SafetyBlock { reason }`,
///   and latched on the callback so the run can also leave a durable receipt
///   (see [`input_block_receipt`])
/// * `on_complete_with_outcome(&outcome)` →
///   `FlowStreamEvent::Complete(outcome.clone())`. Single source of the
///   terminal event — `AgentHarnessRunner::run` no longer sends it via
///   the broadcast channel separately (P4).
///
/// `broadcast::Sender::send` returns an error only when there are zero
/// receivers; we deliberately ignore that since a dropped receiver must not
/// abort the harness loop. The inner harness still produces session events
/// as the canonical log.
pub(super) struct BroadcastCallback {
    tx: broadcast::Sender<FlowStreamEvent>,
    /// Pre-resolved context-window denominator for this run (0 = unknown).
    /// Paired with each `on_context_usage` occupancy so downstream consumers
    /// get a self-contained gauge event without re-resolving the model.
    context_window: u32,
    /// First safety-block reason this run reported, kept so the run can leave a
    /// durable receipt for it. The broadcast frame alone is live-only: it is
    /// gone on reload, invisible to a second tab, and never reaches
    /// `chat.history`.
    ///
    /// First rather than last on purpose. The only block that can reach the
    /// receipt is an input screen (see [`input_block_receipt`]), and that one
    /// ends the run where it fires, so at most one reason exists in the case
    /// that matters; keeping the first means a later cosmetic sanitize notice
    /// cannot overwrite the reason a run actually stopped.
    blocked_reason: Option<String>,
}

impl BroadcastCallback {
    pub(super) const fn new(tx: broadcast::Sender<FlowStreamEvent>, context_window: u32) -> Self {
        Self {
            tx,
            context_window,
            blocked_reason: None,
        }
    }

    /// The safety-block reason latched during the run, if any.
    pub(super) fn blocked_reason(&self) -> Option<&str> {
        self.blocked_reason.as_deref()
    }
}

/// The durable receipt a finished run owes for a safety block, or `None`.
///
/// `on_safety_block` is overloaded — it also fires for an output sanitize
/// (`think.rs`) and for a blocked tool call (`harness::agent::guardrails`) — so
/// the reason alone cannot say what happened. What separates the input screen
/// from its two siblings is that it runs *before* the LLM call and returns the
/// turn immediately: it is the only one that can leave a run finishing `Ok`
/// with nothing said. Both siblings necessarily have an `AssistantMessage`
/// behind them (a sanitize rewrites a response that is then emitted; a tool
/// call is part of one), and an output *block* fails the run outright, so it
/// never reaches this decision at all.
///
/// Known edge: a steering message screened out mid-run, after earlier assistant
/// output in the same run, gets no receipt. That run said something, so it is
/// not the "unanswered user message" shape this receipt exists to cure.
pub(super) fn input_block_receipt(reason: Option<&str>, assistant_messages: u32) -> Option<&str> {
    if assistant_messages == 0 {
        reason
    } else {
        None
    }
}

/// The turn a receipt belongs to: the last `TurnStarted` in the log.
///
/// Same predicate the harness itself uses to file a turn's events
/// (`harness::agent::current_turn_id`). The seeded `TurnStarted` precedes
/// `RunStarted`, so it sits outside the run-scoped window the caller counts
/// assistant messages over — `records` here is the whole log.
fn turn_of_last_start(records: &[SessionEventRecord]) -> Option<TurnId> {
    records.iter().rev().find_map(|r| match &r.event {
        SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
        _ => None,
    })
}

/// Write the durable receipt a guardrail-refused run owes, if it owes one.
///
/// A screened-out input ends the run `Ok`, so the log otherwise reads
/// `UserMessage + TurnStarted + RunStarted + RunFinished{Completed}` — byte
/// identical to a clean empty run — and the reason survives only in the live
/// `SafetyBlock` frame. Every client that attaches afterwards (reload, second
/// tab, room peer, `chat.history`) would see an unanswered user message and no
/// explanation. [`crate::session::projection::project_row`] carries this event
/// on to the `messages` projection those clients read.
///
/// A failed append is logged, not propagated: the run itself succeeded, and
/// losing the receipt must not turn it into a failure.
pub(super) async fn record_input_block(
    service: &dyn SessionService,
    session_id: &SessionId,
    records: &[SessionEventRecord],
    reason: Option<&str>,
    assistant_messages: u32,
) {
    let Some(reason) = input_block_receipt(reason, assistant_messages) else {
        return;
    };
    if let Err(e) = service
        .emit_event(
            session_id,
            SessionEvent::Error {
                turn_id: turn_of_last_start(records),
                kind: ErrorKind::Guardrail,
                message: reason.to_string(),
                // The model cannot retry its way past an input screen; the
                // input itself has to change.
                recoverable: false,
                at: crate::session::events::now_ms(),
            },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to emit input-block receipt");
    }
}

impl HarnessCallback for BroadcastCallback {
    fn on_delta(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Delta(text.to_string()));
    }

    fn on_reasoning(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Reasoning(text.to_string()));
    }

    fn on_tool_call_start(&mut self, id: &str, name: &str, args: &serde_json::Value) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallStart {
            id: id.to_string(),
            name: name.to_string(),
            args: args.clone(),
        });
    }

    fn on_tool_call_done(
        &mut self,
        id: &str,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
        duration_ms: u64,
    ) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallDone {
            id: id.to_string(),
            result: result.cloned(),
            error: error.map(|s| s.to_string()),
            duration_ms,
        });
    }

    fn on_context_usage(&mut self, context_tokens: u32, total_tokens: u64) {
        // Both sides must be known for the panel to render a fraction; a
        // zero window (unresolvable model, no override) keeps the gauge on
        // its terminal `run_complete` cadence only.
        if context_tokens > 0 && self.context_window > 0 {
            let _ = self.tx.send(FlowStreamEvent::ContextGauge {
                context_tokens,
                context_window: self.context_window,
                total_tokens,
            });
        }
    }

    fn on_safety_block(&mut self, reason: &str) {
        if self.blocked_reason.is_none() {
            self.blocked_reason = Some(reason.to_string());
        }
        let _ = self.tx.send(FlowStreamEvent::SafetyBlock {
            reason: reason.to_string(),
        });
    }

    fn on_complete_with_outcome(&mut self, outcome: &FlowOutcome) {
        // P4 (single-source): this is the only place that emits
        // `FlowStreamEvent::Complete` on the broadcast channel. The
        // separate `events.send` previously firing inside
        // `AgentHarnessRunner::run` is gone.
        let _ = self.tx.send(FlowStreamEvent::Complete(outcome.clone()));
    }
}
