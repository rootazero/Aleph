//! Lifecycle callback for `AgentHarness` turn execution.
//!
//! Mirrors the `LoopCallback` surface from the retiring
//! `agent_loop/loop_core.rs` on a narrower contract. The Phase 6a runtime flip
//! (see `docs/superpowers/plans/2026-04-20-managed-agents-phase-6a-runtime-flip.md`)
//! threads implementations of this trait from the Gateway stream sink through
//! the Orchestrator bridge down to the harness, so user-visible delta
//! streaming survives the `AgentLoop → AgentHarness` swap.
//!
//! Today the LLM layer is non-streaming at this seam, so [`HarnessCallback::on_delta`]
//! fires once per assistant turn with the complete text. The trait shape is
//! ready for chunked streaming once `AiProvider::process_stream` is wired.

use std::fmt;

use crate::orchestrator::dispatch::FlowOutcome;

/// Legacy hint pre-dating [`HarnessCallback::on_complete_with_outcome`]
/// taking the full [`FlowOutcome`]. Kept so external callers that
/// referenced it keep compiling; new code should consume the
/// `FlowOutcome` reference directly via the trait method.
#[derive(Debug, Clone, Default)]
#[deprecated(
    since = "0.3.0",
    note = "Use HarnessCallback::on_complete_with_outcome(&FlowOutcome) instead — \
            OutcomeHint only carried hit_limit:bool, FlowOutcome carries the full \
            terminate reason, timeline, and breakdown."
)]
pub struct OutcomeHint {
    pub hit_limit: bool,
}

pub trait HarnessCallback: Send {
    /// Invoked when the harness produces (or forwards) a chunk of assistant
    /// text. May be called multiple times per turn if the upstream LLM layer
    /// streams partial output.
    fn on_delta(&mut self, _text: &str) {}

    /// Invoked when the harness produces a reasoning/thinking fragment.
    fn on_reasoning(&mut self, _text: &str) {}

    /// Invoked once per tool dispatch, *before* the tool executes.
    /// Kept for backward compatibility — prefer `on_tool_call_start`.
    fn on_tool_call(&mut self, _name: &str) {}

    /// Invoked when a tool call begins. `id` pairs with `on_tool_call_done`
    /// and `on_tool_summary`.
    fn on_tool_call_start(&mut self, _id: &str, _name: &str, _args: &serde_json::Value) {}

    /// Invoked when a tool call finishes. `result` and `error` are mutually exclusive.
    fn on_tool_call_done(
        &mut self,
        _id: &str,
        _result: Option<&serde_json::Value>,
        _error: Option<&str>,
    ) {
    }

    /// Invoked with an LLM-generated one-line summary for a tool call.
    fn on_tool_summary(&mut self, _id: &str, _text: &str) {}

    /// Invoked when a safety gate blocks the current turn.
    fn on_safety_block(&mut self, _reason: &str) {}

    /// Invoked when a stop hook blocks the current turn and forces another model turn.
    fn on_stop_hook_block(&mut self, _reason: &str) {}

    /// Invoked when a stop hook halts the loop permanently
    /// (claude-code `preventContinuation` parity). The harness exits with
    /// [`TerminateReason::StopHookHalt`](crate::orchestrator::dispatch::TerminateReason::StopHookHalt)
    /// after firing this callback.
    fn on_stop_hook_halt(&mut self, _reason: &str) {}

    /// Invoked when the primary model is unavailable and a fallback is used.
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}

    /// Invoked when the harness reaches a terminal `TurnState::Done`.
    fn on_complete(&mut self) {}

    /// Invoked by `AgentHarnessRunner` after the inner Think→Act loop
    /// finishes and the full [`FlowOutcome`] has been synthesised from the
    /// harness accessors. The default implementation calls
    /// [`HarnessCallback::on_complete`] for backwards compatibility — the
    /// callback receives no outcome data, matching the legacy behaviour.
    ///
    /// Implementations that need the final terminate reason, token
    /// breakdown, tool timeline, or cost estimate (e.g. the gateway's
    /// `BroadcastCallback`, which fires the terminal
    /// `FlowStreamEvent::Complete(outcome)` from this method) override it.
    ///
    /// Lifecycle ordering — `on_complete_with_outcome` always fires AFTER
    /// `on_complete`. Implementations that emit a terminal "Complete"-style
    /// event should do so here, not in `on_complete`, so the outcome
    /// payload is always present.
    fn on_complete_with_outcome(&mut self, _outcome: &FlowOutcome) {
        self.on_complete();
    }
}

/// Drop-in `HarnessCallback` that ignores every event. Used by call sites
/// that don't need streaming (e.g. `SessionDriver::drive`, unit tests that
/// only assert session-event shape).
#[derive(Default)]
pub struct NoopHarnessCallback;

impl HarnessCallback for NoopHarnessCallback {}

impl fmt::Debug for NoopHarnessCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoopHarnessCallback")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CapturingCallback {
        deltas: Vec<String>,
        tools: Vec<String>,
        completed: bool,
    }

    impl HarnessCallback for CapturingCallback {
        fn on_delta(&mut self, text: &str) {
            self.deltas.push(text.to_string());
        }
        fn on_tool_call(&mut self, name: &str) {
            self.tools.push(name.to_string());
        }
        fn on_complete(&mut self) {
            self.completed = true;
        }
    }

    #[test]
    fn capturing_callback_records_lifecycle() {
        let mut cb = CapturingCallback::default();
        cb.on_delta("hello ");
        cb.on_delta("world");
        cb.on_tool_call("read_file");
        cb.on_complete();
        assert_eq!(cb.deltas, vec!["hello ".to_string(), "world".to_string()]);
        assert_eq!(cb.tools, vec!["read_file".to_string()]);
        assert!(cb.completed);
    }

    #[test]
    fn noop_callback_ignores_all_events() {
        let mut cb = NoopHarnessCallback;
        cb.on_delta("ignored");
        cb.on_tool_call("ignored_tool");
        cb.on_complete();
        // No panic, nothing to assert — the point is absence of side effects.
    }

    #[test]
    fn trait_is_object_safe() {
        fn _use(_: &mut dyn HarnessCallback) {}
        let mut cb = NoopHarnessCallback;
        _use(&mut cb);
    }
}
