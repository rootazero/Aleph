//! `TraceSink` — observability side-channel for `AgentHarness` runs.
//!
//! Events not exposed via `FlowStreamEvent` (internal trace,
//! confirmation prompts, persistence flush) route here instead.

use crate::harness::trace::LoopTraceEvent;

/// Implementations MUST NOT block. The sink is invoked from `AgentHarness`
/// async tasks; blocking calls back-pressure the entire harness loop.
/// Production sinks should push events to an `mpsc` channel and drain
/// elsewhere. The Gateway path uses `GatewayTraceSink` which is mpsc-backed.
pub trait TraceSink: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush(&self);

    /// Stage 7 (#12): emitted once per `HarnessDeps` construction (= once
    /// per session run on the gateway path) for each Stage 1-6 seam. The
    /// `configured` flag distinguishes a wired impl from an explicit
    /// `None` placeholder — Phase-6 will flip these as `aleph.toml`
    /// loading lands.
    ///
    /// Default no-op so existing impls (`NoopTraceSink`, `GatewayTraceSink`,
    /// test sinks) compile unchanged. Test sinks override to capture.
    fn on_init_seam(&self, _stage: &'static str, _seam: &'static str, _configured: bool) {}
}

/// No-op implementation for tests / internal `flow_run` calls.
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush(&self) {}
}
