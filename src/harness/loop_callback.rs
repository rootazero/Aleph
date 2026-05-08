//! LoopCallback — streaming event callback for tool batch execution.
//!
//! Relocated from `agent_loop/loop_core.rs` during Phase 6c deletion.
//! The trait is retained as the seam between `tools::orchestrator::execute_tool_batch`
//! and its callers (currently only the orchestrator's own test suite).

use serde_json::Value;

use crate::harness::trace::{
    LoopTraceEvent, LoopTraceTextKind, ToolCallEndEvent, ToolCallStartEvent,
};
use crate::session::ingress_safety::SafetyError;
use crate::tools::runtime::ToolResult;

/// Callback for streaming events during tool batch execution.
pub trait LoopCallback: Send {
    fn on_trace(&mut self, event: &LoopTraceEvent) {
        match event {
            LoopTraceEvent::TextEmitted { stream, text, .. } => match stream {
                LoopTraceTextKind::Final => self.on_text(text),
                LoopTraceTextKind::Intermediate => self.on_intermediate_text(text),
            },
            LoopTraceEvent::ToolCallStarted { call, .. } => self.on_tool_call_start(call),
            LoopTraceEvent::ToolCallCompleted { call, result, .. } => {
                self.on_tool_call_done(call, result)
            }
            LoopTraceEvent::ToolSummary { summary, .. } => self.on_tool_summary(summary),
            LoopTraceEvent::TurnStarted { .. }
            | LoopTraceEvent::TurnStateEntered { .. }
            | LoopTraceEvent::TurnCompleted { .. }
            | LoopTraceEvent::SessionCompleted { .. } => {}
        }
    }
    fn on_text(&mut self, _text: &str) {}
    fn on_intermediate_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_tool_call_start(&mut self, event: &ToolCallStartEvent) {
        self.on_tool_start(&event.tool_name, &event.input);
    }
    fn on_tool_call_done(&mut self, event: &ToolCallEndEvent, result: &ToolResult) {
        self.on_tool_done(&event.tool_name, result);
    }
    fn on_safety_block(&mut self, _error: &SafetyError) {}
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
    fn on_stop_hook_block(&mut self, _reason: &str) {}
    fn on_stop_hook_error(&mut self, _hook_name: &str, _error: &str) {}
    fn on_tool_summary(&mut self, _summary: &str) {}

    fn on_confirmation_needed(
        &mut self,
        _tool_name: &str,
        _tool_input: &Value,
        _reason: &str,
    ) -> bool {
        false
    }
}

/// No-op callback for when you don't need events.
pub struct NoopCallback;
impl LoopCallback for NoopCallback {}
