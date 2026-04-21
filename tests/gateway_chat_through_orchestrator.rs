//! Task 4c integration test 1: dispatch wiring.
//!
//! A chat-turn request routed through the orchestrator causes exactly one
//! `HarnessRunner::run` invocation. Counter-based assertion.

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::FlowOutcome;
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

#[tokio::test]
async fn dispatch_invokes_harness_runner_once() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();

    let runner = StubHarnessRunner::new(Arc::new(move |ctx| {
        let count = count_clone.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            let outcome = FlowOutcome {
                final_text: "ok".to_string(),
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 0,
                hit_limit: false,
            };
            // Terminal event on the stream so the drain loop exits cleanly.
            let _ = ctx
                .events
                .send(alephcore::orchestrator::FlowStreamEvent::Complete(outcome.clone()));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let response = run_dispatch_and_drain(
        orch,
        basic_request(),
        emitter,
        "run-1",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    assert_eq!(count.load(Ordering::SeqCst), 1, "stub runner invoked once");
    assert_eq!(response, "ok", "FlowOutcome.final_text threaded through");
}
