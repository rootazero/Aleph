//! `GatewayTraceSink` — adapts the harness-side `TraceSink` onto the Gateway's
//! existing trace persistence path.
//!
//! The retiring `StreamCallback::flush_trace_persistence` drove
//! `TracePersistence::flush` asynchronously. That machinery lives inside
//! `run_loop.rs` and stays valid under the flip; this adapter bridges the
//! orchestrator-side flush invocation to the same persistence layer by
//! holding a shared `TraceFlushHandle` trait object.

use std::sync::Arc;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// Minimal abstraction over "the trace persistence actor". Implementations
/// hold whatever state is required to persist traces (e.g.
/// `StateDatabase` + task id); this trait is intentionally small so the
/// adapter stays a thin forwarder.
///
/// The existing `StreamCallback` retains its own persistence path during the
/// 4c flip — this sink is a parallel hook for the orchestrator drain so
/// production code can call `trace_sink.flush()` once `AgentHarnessRunner::run`
/// completes (harness_bridge.rs already does this).
pub trait TraceFlushHandle: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush_blocking(&self);
}

/// No-op handle used when Gateway has no state database to persist into.
#[allow(dead_code)] // wired by boot path when state_database is None
pub struct NoopTraceFlushHandle;

impl TraceFlushHandle for NoopTraceFlushHandle {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush_blocking(&self) {}
}

/// `TraceSink` implementation that forwards to a `TraceFlushHandle`.
///
/// Constructor takes:
/// * `handle` — the actual persistence driver (or `NoopTraceFlushHandle`).
///
/// `on_trace` delegates to `handle.on_trace`; `flush` invokes
/// `handle.flush_blocking`.  The Gateway wires one of these per-run when
/// building a `FlowRequest`.
pub struct GatewayTraceSink {
    handle: Arc<dyn TraceFlushHandle>,
}

impl GatewayTraceSink {
    #[allow(dead_code)]
    pub fn new(handle: Arc<dyn TraceFlushHandle>) -> Self {
        Self { handle }
    }

    /// Convenience: build a noop sink when no persistence target is configured.
    #[allow(dead_code)] // called by future boot paths without state_database
    pub fn noop() -> Self {
        Self {
            handle: Arc::new(NoopTraceFlushHandle),
        }
    }
}

impl TraceSink for GatewayTraceSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.handle.on_trace(event);
    }

    fn flush(&self) {
        self.handle.flush_blocking();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct CountingHandle {
        traces: AtomicUsize,
        flushed: AtomicBool,
    }

    impl TraceFlushHandle for CountingHandle {
        fn on_trace(&self, _event: &LoopTraceEvent) {
            self.traces.fetch_add(1, Ordering::SeqCst);
        }
        fn flush_blocking(&self) {
            self.flushed.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn sink_forwards_on_trace_and_flush() {
        let handle = Arc::new(CountingHandle {
            traces: AtomicUsize::new(0),
            flushed: AtomicBool::new(false),
        });
        let sink = GatewayTraceSink::new(handle.clone());

        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
        sink.flush();

        assert_eq!(handle.traces.load(Ordering::SeqCst), 1);
        assert!(handle.flushed.load(Ordering::SeqCst));
    }

    #[test]
    fn noop_sink_compiles_and_runs() {
        let sink = GatewayTraceSink::noop();
        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
        sink.flush();
    }
}
