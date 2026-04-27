//! Task 4c integration test 3: stream tool events in order.
//!
//! Stub harness emits `ToolCallStart → ToolCallDone → ToolSummary` on the
//! broadcast channel; the emitter must observe all three, in order, as
//! their corresponding `StreamEvent`s.

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
            // Emit in-order: Start → Done → Summary → Complete.
            let _ = ctx.events.send(FlowStreamEvent::ToolCallStart {
                id: "call-1".to_string(),
                name: "search".to_string(),
                args: serde_json::json!({ "q": "rust" }),
            });
            let _ = ctx.events.send(FlowStreamEvent::ToolCallDone {
                id: "call-1".to_string(),
                result: Some(serde_json::json!({ "hits": 3 })),
                error: None,
            });
            let _ = ctx.events.send(FlowStreamEvent::ToolSummary {
                id: "call-1".to_string(),
                text: "found 3 hits".to_string(),
            });

            // Give the drain task a moment to observe before Complete.
            tokio::task::yield_now().await;

            let outcome = FlowOutcome {
                final_text: "done".to_string(),
                iterations: 1,
                tool_calls_made: 1,
                total_tokens: 0,
                hit_limit: false,
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
    // Filter to the three we care about and assert order.
    let names: Vec<&'static str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolStart { .. } => Some("tool_start"),
            StreamEvent::ToolEnd { .. } => Some("tool_end"),
            StreamEvent::ToolUpdate { .. } => Some("tool_update"),
            _ => None,
        })
        .collect();

    assert_eq!(
        names,
        vec!["tool_start", "tool_end", "tool_update"],
        "expected tool events in start/done/summary order; got: {names:?} (all events: {events:?})"
    );
}
