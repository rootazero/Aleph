//! `TraceSink` — observability side-channel for `AgentHarness` runs.
//!
//! Events not exposed via `FlowStreamEvent` (internal trace,
//! confirmation prompts, persistence flush) route here instead.

use crate::harness::trace::LoopTraceEvent;

/// Implementations MUST NOT block. The sink is invoked from `AgentHarness`
/// async tasks; blocking calls back-pressure the entire harness loop, so
/// production sinks return before any write happens. The Gateway path uses
/// `GatewayTraceSink`, which forwards synchronously into
/// `TracePersistence::record`; that spawns one write task per event and keeps
/// its handle for `flush`, so the harness-side call never blocks.
pub trait TraceSink: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush(&self);
}

/// No-op implementation for tests / internal `flow_run` calls.
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush(&self) {}
}
