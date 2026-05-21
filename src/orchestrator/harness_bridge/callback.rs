//! Broadcast callback adapter that fans `HarnessCallback` lifecycle events
//! onto the orchestrator's `FlowStreamEvent` broadcast channel.

use crate::harness::callback::HarnessCallback;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
use tokio::sync::broadcast;

/// Adapter that fans `HarnessCallback` lifecycle events onto the
/// orchestrator's `FlowStreamEvent` broadcast channel.
///
/// * `on_delta(text)` → `FlowStreamEvent::Delta(text)`
/// * `on_reasoning(text)` → `FlowStreamEvent::Reasoning(text)`
/// * `on_tool_call(name)` → `FlowStreamEvent::ToolCallStart { id: "legacy", name, args: null }`
/// * `on_tool_call_start(id, name, args)` → `FlowStreamEvent::ToolCallStart { id, name, args }`
/// * `on_tool_call_done(id, result, error)` → `FlowStreamEvent::ToolCallDone { id, result, error }`
/// * `on_tool_summary(id, text)` → `FlowStreamEvent::ToolSummary { id, text }`
/// * `on_safety_block(reason)` → `FlowStreamEvent::SafetyBlock { reason }`
/// * `on_stop_hook_block(reason)` → `FlowStreamEvent::StopHookBlock { reason }`
/// * `on_model_fallback(reason, fallback_model)` → `FlowStreamEvent::ModelFallback { reason, fallback_model }`
/// * `on_complete()` → no-op (the terminal `Complete(outcome)` event is
///   emitted by [`BroadcastCallback::on_complete_with_outcome`], which fires
///   strictly after `on_complete` and after `AgentHarnessRunner` has built
///   the full `FlowOutcome`)
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
}

impl BroadcastCallback {
    pub(super) fn new(tx: broadcast::Sender<FlowStreamEvent>) -> Self {
        Self { tx }
    }
}

impl HarnessCallback for BroadcastCallback {
    fn on_delta(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Delta(text.to_string()));
    }

    fn on_reasoning(&mut self, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::Reasoning(text.to_string()));
    }

    /// Legacy compatibility shim — fires `ToolCallStart` with a synthetic id.
    /// Prefer `on_tool_call_start` for structured tool events.
    fn on_tool_call(&mut self, name: &str) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallStart {
            id: "legacy".to_string(),
            name: name.to_string(),
            args: serde_json::Value::Null,
        });
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
    ) {
        let _ = self.tx.send(FlowStreamEvent::ToolCallDone {
            id: id.to_string(),
            result: result.cloned(),
            error: error.map(|s| s.to_string()),
        });
    }

    fn on_tool_summary(&mut self, id: &str, text: &str) {
        let _ = self.tx.send(FlowStreamEvent::ToolSummary {
            id: id.to_string(),
            text: text.to_string(),
        });
    }

    fn on_safety_block(&mut self, reason: &str) {
        let _ = self.tx.send(FlowStreamEvent::SafetyBlock {
            reason: reason.to_string(),
        });
    }

    fn on_stop_hook_block(&mut self, reason: &str) {
        let _ = self.tx.send(FlowStreamEvent::StopHookBlock {
            reason: reason.to_string(),
        });
    }

    fn on_model_fallback(&mut self, reason: &str, fallback_model: &str) {
        let _ = self.tx.send(FlowStreamEvent::ModelFallback {
            reason: reason.to_string(),
            fallback_model: fallback_model.to_string(),
        });
    }

    // `on_complete` is intentionally a no-op here.
    // The terminal `Complete(outcome)` event is emitted in
    // `on_complete_with_outcome` (P4), so the broadcast channel always
    // sees the full outcome payload.
    fn on_complete(&mut self) {}

    fn on_complete_with_outcome(&mut self, outcome: &FlowOutcome) {
        // P4 (single-source): this is the only place that emits
        // `FlowStreamEvent::Complete` on the broadcast channel. The
        // separate `events.send` previously firing inside
        // `AgentHarnessRunner::run` is gone.
        let _ = self.tx.send(FlowStreamEvent::Complete(outcome.clone()));
    }
}
