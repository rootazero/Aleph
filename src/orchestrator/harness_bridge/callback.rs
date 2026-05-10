//! Broadcast callback adapter that fans `HarnessCallback` lifecycle events
//! onto the orchestrator's `FlowStreamEvent` broadcast channel.

use crate::harness::callback::HarnessCallback;
use crate::orchestrator::dispatch::FlowStreamEvent;
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
/// * `on_complete()` → no-op (`Complete(outcome)` is emitted by `AgentHarnessRunner::run`)
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
    // `AgentHarnessRunner::run` emits `Complete(outcome)` after synthesising
    // the full `FlowOutcome`, ensuring it is always the last event on the
    // broadcast channel (see Task 1 plan §Step 3).
    fn on_complete(&mut self) {}
}
