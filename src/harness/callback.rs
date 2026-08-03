//! Lifecycle callback for `AgentHarness` turn execution.
//!
//! The Phase 6a runtime flip
//! (see `docs/superpowers/plans/2026-04-20-managed-agents-phase-6a-runtime-flip.md`)
//! threads implementations of this trait from the Gateway stream sink through
//! the Orchestrator bridge down to the harness, so user-visible delta
//! streaming survives the legacy `AgentLoop` → `AgentHarness` swap.
//!
//! When the active provider exposes an HTTP delta seam and no output guardrail
//! is wired, the harness streams live: [`HarnessCallback::on_delta`] /
//! [`HarnessCallback::on_reasoning`] fire incrementally as tokens arrive (see
//! `AgentHarness::stream_llm_call`). For non-HTTP providers (mocks, non-streaming
//! backends) and guardrailed turns — where the output guardrail must sanitise
//! the final text before any emission — `on_delta` instead fires once per turn
//! with the complete text. Consumers must therefore tolerate either cadence.

use std::fmt;

use crate::orchestrator::dispatch::FlowOutcome;

pub trait HarnessCallback: Send {
    /// Invoked when the harness produces (or forwards) a chunk of assistant
    /// text. May be called multiple times per turn if the upstream LLM layer
    /// streams partial output.
    fn on_delta(&mut self, _text: &str) {}

    /// Invoked when the harness produces a reasoning/thinking fragment.
    fn on_reasoning(&mut self, _text: &str) {}

    /// Invoked when a tool call begins. `id` pairs with `on_tool_call_done`.
    fn on_tool_call_start(&mut self, _id: &str, _name: &str, _args: &serde_json::Value) {}

    /// Invoked when a tool call finishes. `result` and `error` are mutually
    /// exclusive. `duration_ms` is the tool's measured wall-clock execution
    /// time (0 for a within-batch memo hit, which re-executes nothing).
    fn on_tool_call_done(
        &mut self,
        _id: &str,
        _result: Option<&serde_json::Value>,
        _error: Option<&str>,
        _duration_ms: u64,
    ) {
    }

    /// Invoked once per LLM call, right after the provider-billed token usage
    /// is folded into the run totals. `context_tokens` is the call's
    /// context-window occupancy (prompt + generated; see
    /// `TokenUsage::context_occupancy_tokens`), `total_tokens` the run's
    /// cumulative billed total so far. Lets consumers stream a live
    /// occupancy gauge — the value drops on the call right after a mid-run
    /// compaction. Default no-op.
    fn on_context_usage(&mut self, _context_tokens: u32, _total_tokens: u64) {}

    /// Invoked when a safety gate blocks the current turn.
    fn on_safety_block(&mut self, _reason: &str) {}

    /// Invoked by `AgentHarnessRunner` after the inner Think→Act loop
    /// finishes and the full [`FlowOutcome`] has been synthesised from the
    /// harness accessors. This is the single terminal hook.
    ///
    /// Implementations that need the final terminate reason, token
    /// breakdown, tool timeline, or cost estimate (e.g. the gateway's
    /// `BroadcastCallback`, which fires the terminal
    /// `FlowStreamEvent::Complete(outcome)` from this method) override it.
    ///
    /// Its argument-free twin `on_complete` was deleted: the harness loop
    /// fired it from eight places, the one production impl overrode it with an
    /// explicitly empty body, and everything terminal already rides the
    /// outcome payload.
    fn on_complete_with_outcome(&mut self, _outcome: &FlowOutcome) {}
}

/// Drop-in `HarnessCallback` that ignores every event. Used by call sites
/// that don't need streaming (e.g. unit tests that only assert session-event
/// shape).
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
        fn on_tool_call_start(&mut self, _id: &str, name: &str, _args: &serde_json::Value) {
            self.tools.push(name.to_string());
        }
        fn on_complete_with_outcome(&mut self, _outcome: &FlowOutcome) {
            self.completed = true;
        }
    }

    #[test]
    fn capturing_callback_records_lifecycle() {
        let mut cb = CapturingCallback::default();
        cb.on_delta("hello ");
        cb.on_delta("world");
        cb.on_tool_call_start("call-1", "read_file", &serde_json::Value::Null);
        cb.on_complete_with_outcome(&FlowOutcome::default());
        assert_eq!(cb.deltas, vec!["hello ".to_string(), "world".to_string()]);
        assert_eq!(cb.tools, vec!["read_file".to_string()]);
        assert!(cb.completed);
    }

    #[test]
    fn noop_callback_ignores_all_events() {
        let mut cb = NoopHarnessCallback;
        cb.on_delta("ignored");
        cb.on_tool_call_start("call-1", "ignored_tool", &serde_json::Value::Null);
        cb.on_complete_with_outcome(&FlowOutcome::default());
        // No panic, nothing to assert — the point is absence of side effects.
    }

    #[test]
    fn trait_is_object_safe() {
        fn _use(_: &mut dyn HarnessCallback) {}
        let mut cb = NoopHarnessCallback;
        _use(&mut cb);
    }
}
