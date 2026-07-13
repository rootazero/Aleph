//! Task 4c integration test 3: stream tool events in order.
//!
//! Stub harness emits `ToolCallStart → ToolCallDone` on the broadcast
//! channel; the emitter must observe both, in order, as their
//! corresponding `StreamEvent`s, followed by the terminal `RunComplete`.

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::Arc;

use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter, StreamEvent};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::{FlowOutcome, FlowStreamEvent};
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

#[tokio::test]
async fn tool_events_preserve_order() {
    let runner = StubHarnessRunner::new(Arc::new(|ctx| {
        Box::pin(async move {
            // Emit in-order: Start → Done → Complete.
            let _ = ctx.events.send(FlowStreamEvent::ToolCallStart {
                id: "call-1".to_string(),
                name: "search".to_string(),
                args: serde_json::json!({ "q": "rust" }),
            });
            let _ = ctx.events.send(FlowStreamEvent::ToolCallDone {
                id: "call-1".to_string(),
                result: Some(serde_json::json!({ "hits": 3 })),
                error: None,
                duration_ms: 12,
            });

            // Give the drain task a moment to observe before Complete.
            tokio::task::yield_now().await;

            let outcome = FlowOutcome {
                final_text: "done".to_string(),
                iterations: 1,
                tool_calls_made: 1,
                total_tokens: 0,
                hit_limit: false,
                ..Default::default()
            };
            let _ = ctx.events.send(FlowStreamEvent::Complete(outcome.clone()));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let collector = Arc::new(CollectingEventEmitter::new());
    let emitter: Arc<dyn EventEmitter> = collector.clone();

    let _ = run_dispatch_and_drain(
        orch,
        basic_request(),
        emitter,
        "run-3",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    let events = collector.events().await;
    // Filter to the events we care about and assert order. RunComplete is
    // single-sourced from the drain, so exactly one must follow the pair.
    let names: Vec<&'static str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolStart { .. } => Some("tool_start"),
            StreamEvent::ToolEnd { .. } => Some("tool_end"),
            StreamEvent::RunComplete { .. } => Some("run_complete"),
            _ => None,
        })
        .collect();

    assert_eq!(
        names,
        vec!["tool_start", "tool_end", "run_complete"],
        "expected start/done order then a single RunComplete; got: {names:?} (all events: {events:?})"
    );
}
