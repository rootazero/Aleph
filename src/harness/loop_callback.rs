//! LoopCallback — streaming event callback for tool batch execution.
//!
//! Trait surface kept available for callers that want to observe
//! `LoopTraceEvent`s without owning a trace sink. `NoopCallback` is the
//! drop-in default. The previous primary consumer (`tools::orchestrator::execute_tool_batch`)
//! has been dissolved; the trait is preserved for future revival without
//! breaking the export surface.

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
            | LoopTraceEvent::SessionCompleted { .. }
            | LoopTraceEvent::WorktreeCreated { .. }
            | LoopTraceEvent::WorktreeCleanedUp { .. }
            | LoopTraceEvent::McpScopeAttached { .. }
            | LoopTraceEvent::McpScopeCleaned { .. }
            | LoopTraceEvent::ProviderUsage { .. } => { /* observability passthrough */ }
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
    fn on_stop_hook_halt(&mut self, _reason: &str) {}
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
