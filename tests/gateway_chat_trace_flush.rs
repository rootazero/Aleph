//! Task 4c integration test 6: TraceSink::flush fires after dispatch completes.
//!
//! `AgentHarnessRunner::run` (via the real harness-bridge path) calls
//! `trace_sink.flush()` when the inner run ends. Our stub `HarnessRunner`
//! doesn't auto-call flush (it's not `AgentHarnessRunner`), so to verify the
//! helper path we let the runner itself call `flush()` on the received sink —
//! mirroring what `AgentHarnessRunner` does in production. The test then
//! observes the test sink's flush flag.

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::harness::{trace::LoopTraceEvent, TraceSink};
use alephcore::orchestrator::{FlowOutcome, FlowStreamEvent};
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

struct TestTraceSink {
    flush_called: Arc<AtomicBool>,
}

impl TraceSink for TestTraceSink {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush(&self) {
        self.flush_called.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn trace_sink_flush_called_after_run() {
    let flushed = Arc::new(AtomicBool::new(false));
    let flushed_clone = flushed.clone();

    let runner = StubHarnessRunner::new(Arc::new(move |ctx| {
        Box::pin(async move {
            // Stand in for AgentHarnessRunner's own flush call at the end of run.
            if let Some(sink) = ctx.trace_sink.as_ref() {
                sink.flush();
            }
            let outcome = FlowOutcome {
                final_text: "done".to_string(),
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 0,
                hit_limit: false,
            };
            let _ = ctx
                .events
                .send(FlowStreamEvent::Complete(outcome.clone()));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let mut req = basic_request();
    req.trace_sink = Some(Arc::new(TestTraceSink {
        flush_called: flushed_clone,
    }) as Arc<dyn TraceSink>);

    let _ = run_dispatch_and_drain(
        orch,
        req,
        emitter,
        "run-6",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    assert!(
        flushed.load(Ordering::SeqCst),
        "TraceSink::flush must be called after dispatch completes"
    );
}
