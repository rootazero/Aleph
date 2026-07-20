//! Callback adapter that bridges `AgentLoop` events to Gateway `StreamEvents`.

use crate::sync_primitives::{Arc, AtomicU32, Mutex, Ordering};

/// Persists trace events to the state database.
pub(super) struct TracePersistence {
    db: Arc<crate::resilience::StateDatabase>,
    task_id: String,
    next_step_index: AtomicU32,
    pending_writes: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl TracePersistence {
    pub(super) const fn new(db: Arc<crate::resilience::StateDatabase>, task_id: String) -> Self {
        Self {
            db,
            task_id,
            next_step_index: AtomicU32::new(0),
            pending_writes: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn record(&self, event: &crate::harness::trace::LoopTraceEvent) {
        let step_index = self.next_step_index.fetch_add(1, Ordering::Relaxed);
        let db = self.db.clone();
        let task_id = self.task_id.clone();
        let trace_event: aleph_protocol::AgentTraceEvent = event.clone().into();

        let handle = tokio::spawn(async move {
            let trace = crate::resilience::TaskTrace::new(task_id.clone(), step_index, trace_event);
            if let Err(error) = db.insert_trace(&trace).await {
                tracing::warn!(
                    task_id = %task_id,
                    step_index,
                    error = %error,
                    "Failed to persist task trace"
                );
            }
        });

        self.pending_writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    pub(super) async fn flush(&self) {
        let handles = {
            let mut pending = self
                .pending_writes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *pending)
        };

        for handle in handles {
            if let Err(error) = handle.await {
                tracing::warn!(error = %error, "Task trace persistence task failed");
            }
        }
    }
}

/// Shared state for the stream callback.
pub(super) struct StreamCallbackState {
    trace_persistence: Option<Arc<TracePersistence>>,
}

impl StreamCallbackState {
    pub(super) const fn new(trace_persistence: Option<Arc<TracePersistence>>) -> Self {
        Self { trace_persistence }
    }

    pub(super) fn persist_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.record(event);
        }
    }

    pub(super) async fn flush_trace_persistence(&self) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.flush().await;
        }
    }
}

/// Adapter that translates the existing `StreamCallbackState` into the
/// `TraceFlushHandle` the orchestrator-side `GatewayTraceSink` expects.
///
/// Keeps `TracePersistence::flush` wired — same persistence path as the
/// retiring `StreamCallback::flush_trace_persistence`. `on_trace` routes each
/// `LoopTraceEvent` into the same queue.
pub(super) struct CallbackStateFlushHandle {
    state: Arc<StreamCallbackState>,
}

impl CallbackStateFlushHandle {
    pub(super) const fn new(state: Arc<StreamCallbackState>) -> Self {
        Self { state }
    }
}

impl super::trace_sink_adapter::TraceFlushHandle for CallbackStateFlushHandle {
    fn on_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
        // `agent_loop::LoopTraceEvent` is a re-export of `harness::trace::LoopTraceEvent`
        // (see `src/agent_loop/trace.rs`), so no translation needed today. The
        // trait boundary is kept distinct in case Phase 6c splits them.
        self.state.persist_trace(event);
    }

    fn flush_blocking(&self) {
        // Fire-and-forget blocking spawn; the existing flush is async and
        // returns only after all pending persistence handles drain.
        let state = self.state.clone();
        // We can't `.await` in a non-async function; use a current-thread
        // tokio handle if available. If no runtime is active (tests), the
        // block_on is inert since there are no pending handles to flush.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                state.flush_trace_persistence().await;
            });
        }
    }
}
