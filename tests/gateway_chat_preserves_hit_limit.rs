//! Task 4c integration test 2: hit_limit → i18n ErrLoopExhausted.
//!
//! A `FlowOutcome` with `hit_limit: true` and empty `final_text` must produce
//! the localized loop-exhausted message, preserving parity with the retiring
//! `run_loop.rs` branch.

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::Arc;

use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::FlowOutcome;
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

#[tokio::test]
async fn hit_limit_with_empty_text_emits_i18n_message() {
    let runner = StubHarnessRunner::new(Arc::new(|ctx| {
        Box::pin(async move {
            let outcome = FlowOutcome {
                final_text: String::new(),
                iterations: 42,
                tool_calls_made: 17,
                total_tokens: 0,
                hit_limit: true,
            };
            let _ = ctx
                .events
                .send(alephcore::orchestrator::FlowStreamEvent::Complete(
                    outcome.clone(),
                ));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let response = run_dispatch_and_drain(
        orch,
        basic_request(),
        emitter,
        "run-2",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    // Reference string: i18n `ErrLoopExhausted` for English. Match the stable
    // marker substring so minor wording tweaks don't break the test.
    assert!(
        response.contains("unable to complete"),
        "expected i18n loop-exhausted message, got: {response}"
    );
    assert!(
        response.contains("42 iterations"),
        "expected iteration count in response, got: {response}"
    );
    assert!(
        response.contains("17 tool calls"),
        "expected tool-call count in response, got: {response}"
    );
}

#[tokio::test]
async fn hit_limit_with_text_uses_text() {
    // Preserves the run_loop.rs branch: even when hit_limit is true, if the
    // model produced final text, return that text (not the exhaustion string).
    let runner = StubHarnessRunner::new(Arc::new(|ctx| {
        Box::pin(async move {
            let outcome = FlowOutcome {
                final_text: "partial answer".to_string(),
                iterations: 5,
                tool_calls_made: 2,
                total_tokens: 0,
                hit_limit: true,
            };
            let _ = ctx
                .events
                .send(alephcore::orchestrator::FlowStreamEvent::Complete(
                    outcome.clone(),
                ));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let response = run_dispatch_and_drain(
        orch,
        basic_request(),
        emitter,
        "run-2b",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    assert_eq!(response, "partial answer");
}
