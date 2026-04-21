//! TraceSink — observability side-channel for AgentHarness runs.
//!
//! Events not exposed via `FlowStreamEvent` (internal trace,
//! confirmation prompts, persistence flush) route here instead.

use crate::harness::trace::LoopTraceEvent;

pub trait TraceSink: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush(&self);
}

/// No-op implementation for tests / internal flow_run calls.
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush(&self) {}
}
