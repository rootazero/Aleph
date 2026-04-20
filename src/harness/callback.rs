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

pub trait HarnessCallback: Send {
    /// Invoked when the harness produces (or forwards) a chunk of assistant
    /// text. May be called multiple times per turn if the upstream LLM layer
    /// streams partial output.
    fn on_delta(&mut self, text: &str);

    /// Invoked once per tool dispatch, *before* the tool executes.
    fn on_tool_call(&mut self, name: &str);

    /// Invoked when the harness reaches a terminal `TurnState::Done`.
    fn on_complete(&mut self);
}

/// Drop-in `HarnessCallback` that ignores every event. Used by call sites
/// that don't need streaming (e.g. `SessionDriver::drive`, unit tests that
/// only assert session-event shape).
#[derive(Default)]
pub struct NoopHarnessCallback;

impl HarnessCallback for NoopHarnessCallback {
    fn on_delta(&mut self, _text: &str) {}
    fn on_tool_call(&mut self, _name: &str) {}
    fn on_complete(&mut self) {}
}

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
