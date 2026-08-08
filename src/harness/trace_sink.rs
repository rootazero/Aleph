//! `TraceSink` — observability side-channel for `AgentHarness` runs.
//!
//! Events not exposed via `FlowStreamEvent` (internal trace,
//! confirmation prompts, persistence flush) route here instead.

use crate::harness::trace::LoopTraceEvent;

/// Implementations MUST NOT block. The sink is invoked from `AgentHarness`
/// async tasks; blocking calls back-pressure the entire harness loop.
/// Production sinks should push events to an `mpsc` channel and drain
/// elsewhere. The Gateway path uses `GatewayTraceSink`, which forwards
/// synchronously into `TracePersistence`'s own mpsc-backed queue (drained
/// asynchronously), so the harness-side call still never blocks.
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
