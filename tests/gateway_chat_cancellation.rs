//! Task 4c integration test 5: cancellation token propagates cleanly.
//!
//! Caller cancels mid-dispatch → the drain task exits without panic and the
//! helper returns an `ExecutionError` (not a runtime panic).

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::Duration;

use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::FlowError;
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

#[tokio::test]
async fn cancel_mid_dispatch_exits_cleanly() {
    let runner = StubHarnessRunner::new(Arc::new(|ctx| {
        Box::pin(async move {
            // Wait until cancel fires, then return a Cancelled FlowError so the
            // helper can translate to an appropriate ExecutionError without
            // panicking and without the drain task wedging.
            ctx.cancel.cancelled().await;
            Err(FlowError::Cancelled)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());
    let cancel = CancellationToken::new();

    // Cancel shortly after dispatch begins.
    let cancel_trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_trigger.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_dispatch_and_drain(
            orch,
            basic_request(),
            emitter,
            "run-5",
            cancel,
            Locale::En,
        ),
    )
    .await;

    // Timeout must not fire — the helper must return quickly after cancel.
    let outer = result.expect("helper must resolve within 2s after cancel");
    // The helper returned (no panic), and the underlying flow error was cancelled.
    // Accept either Err(ExecutionError) or Ok with any text — what matters is
    // the drain exited cleanly. We assert it was an error path since cancel
    // produces FlowError::Cancelled.
    assert!(outer.is_err(), "cancel mid-dispatch must surface as error; got: {outer:?}");
}
