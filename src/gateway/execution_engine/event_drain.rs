//! Event drain: maps `FlowStreamEvent` variants to `EventEmitter` calls.
//!
//! This is a pure, stateless-per-call helper extracted from the in-progress
//! StreamCallback. It holds drain-side state in `DrainState` (pending tool
//! pairs, first-delta flag) so callers don't need to replicate that logic.
//!
//! Task 4c will wire this into `run_loop.rs`; for now it is only unit-tested.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tokio::sync::Mutex;
use tracing::trace;

use crate::gateway::event_emitter::{EventEmitError, EventEmitter, RunSummary, StreamEvent};
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};

/// Pending tool call info stashed between `ToolCallStart` and `ToolCallDone`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // used by Task 4c when wired into run_loop.rs
pub(crate) struct PendingTool {
    pub name: String,
}

/// Mutable drain state shared across calls for a single run.
#[derive(Debug, Default)]
#[allow(dead_code)] // used by Task 4c when wired into run_loop.rs
pub(crate) struct DrainState {
    /// Set to `true` on the first `Delta` event so callers can detect
    /// "first-delta" transitions if needed in the future.
    pub has_emitted_text: bool,
    /// Pending tool calls keyed by tool-call id.
    pub pending_tools: HashMap<String, PendingTool>,
}

/// Map one `FlowStreamEvent` to the appropriate `EventEmitter` call(s).
///
/// `run_id` is forwarded verbatim into every `StreamEvent` variant.
/// `state` accumulates cross-event bookkeeping (pending tools, first-delta).
///
/// This function is `async` because the `EventEmitter` trait is async.
#[allow(dead_code)] // wired into run_loop.rs by Task 4c
pub(crate) async fn emit_flow_event(
    event: FlowStreamEvent,
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    state: &Arc<Mutex<DrainState>>,
) -> Result<(), EventEmitError> {
    match event {
        FlowStreamEvent::Delta(text) => {
            let seq = emitter.next_seq();
            {
                let mut s = state.lock().await;
                s.has_emitted_text = true;
            }
            emitter
                .emit(StreamEvent::ResponseChunk {
                    run_id: run_id.to_string(),
                    seq,
                    delta: text.clone(),
                    content: text.clone(), // backward-compat alias
                    full_text: String::new(),
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: false,
                })
                .await?;
        }

        FlowStreamEvent::Reasoning(text) => {
            // Map to the existing Reasoning stream event.
            // TODO(task-4c): verify emitter channel forwards Reasoning to clients.
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::Reasoning {
                    run_id: run_id.to_string(),
                    seq,
                    content: text,
                    is_complete: false,
                })
                .await?;
        }

        FlowStreamEvent::ToolCallStart { id, name, args } => {
            {
                let mut s = state.lock().await;
                s.pending_tools
                    .insert(id.clone(), PendingTool { name: name.clone() });
            }
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::ToolStart {
                    run_id: run_id.to_string(),
                    seq,
                    tool_name: name,
                    tool_id: id,
                    params: args,
                })
                .await?;
        }

        FlowStreamEvent::ToolCallDone { id, result, error } => {
            {
                let mut s = state.lock().await;
                s.pending_tools.remove(&id);
            }
            let tool_result = if let Some(err) = error {
                crate::gateway::event_emitter::ToolResult::error(err)
            } else {
                let output = result.map(|v| v.to_string()).unwrap_or_default();
                crate::gateway::event_emitter::ToolResult::success(output)
            };
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::ToolEnd {
                    run_id: run_id.to_string(),
                    seq,
                    tool_id: id,
                    result: tool_result,
                    duration_ms: 0,
                })
                .await?;
        }

        FlowStreamEvent::ToolSummary { id, text } => {
            // No dedicated ToolSummary StreamEvent today; emit as ToolUpdate.
            // TODO(task-4c): add StreamEvent::ToolSummary variant or re-evaluate.
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::ToolUpdate {
                    run_id: run_id.to_string(),
                    seq,
                    tool_id: id,
                    progress: text,
                })
                .await?;
        }

        FlowStreamEvent::SafetyBlock { reason } => {
            // Map safety blocks to a run error with a recognisable error code.
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::RunError {
                    run_id: run_id.to_string(),
                    seq,
                    error: reason,
                    error_code: Some("safety_block".to_string()),
                })
                .await?;
        }

        FlowStreamEvent::StopHookBlock { reason } => {
            // Stop-hook blocks are informational; trace-log and continue.
            // TODO(task-4c): surface as a dedicated StreamEvent if UI needs it.
            trace!(
                run_id,
                reason,
                "stop_hook_block: harness will force another turn"
            );
        }

        FlowStreamEvent::ModelFallback {
            reason,
            fallback_model,
        } => {
            // Emit via RunError with a distinct code so the client can show a
            // non-fatal fallback indicator.
            // TODO(task-4c): consider a dedicated StreamEvent::ModelFallback.
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::RunError {
                    run_id: run_id.to_string(),
                    seq,
                    error: format!("{reason} (fallback: {fallback_model})"),
                    error_code: Some("model_fallback".to_string()),
                })
                .await?;
        }

        FlowStreamEvent::Complete(outcome) => {
            emit_complete(emitter, run_id, &outcome).await?;
        }
    }
    Ok(())
}

/// Helper: emit the terminal `RunComplete` event from a `FlowOutcome`.
#[allow(dead_code)] // called from emit_flow_event which is itself dead_code until Task 4c
async fn emit_complete(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    outcome: &FlowOutcome,
) -> Result<(), EventEmitError> {
    let summary = RunSummary {
        total_tokens: u64::from(outcome.total_tokens),
        tool_calls: outcome.tool_calls_made,
        loops: outcome.iterations,
        final_response: if outcome.final_text.is_empty() {
            None
        } else {
            Some(outcome.final_text.clone())
        },
    };
    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq,
            summary,
            total_duration_ms: 0,
        })
        .await
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::CollectingEventEmitter;

    fn make_state() -> Arc<Mutex<DrainState>> {
        Arc::new(Mutex::new(DrainState::default()))
    }

    /// Build a `CollectingEventEmitter` and an `Arc<dyn EventEmitter>` that
    /// points to the same allocation so we can inspect events after the call.
    fn make_emitter() -> (Arc<CollectingEventEmitter>, Arc<dyn EventEmitter>) {
        let inner = Arc::new(CollectingEventEmitter::new());
        let dyn_ref: Arc<dyn EventEmitter> = inner.clone();
        (inner, dyn_ref)
    }

    #[tokio::test]
    async fn delta_goes_to_emitter_text_delta() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        emit_flow_event(
            FlowStreamEvent::Delta("hello".to_string()),
            &emitter,
            "run-1",
            &state,
        )
        .await
        .expect("emit ok");

        let events = inner.events().await;

        assert_eq!(events.len(), 1, "exactly one event emitted");
        match &events[0] {
            StreamEvent::ResponseChunk { delta, run_id, .. } => {
                assert_eq!(delta, "hello");
                assert_eq!(run_id, "run-1");
            }
            other => panic!("expected ResponseChunk, got {other:?}"),
        }
        assert!(state.lock().await.has_emitted_text, "flag set after delta");
    }

    #[tokio::test]
    async fn tool_call_start_and_done_pair() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        emit_flow_event(
            FlowStreamEvent::ToolCallStart {
                id: "tc-1".to_string(),
                name: "search".to_string(),
                args: serde_json::json!({ "q": "rust" }),
            },
            &emitter,
            "run-2",
            &state,
        )
        .await
        .expect("start ok");

        {
            let s = state.lock().await;
            assert!(s.pending_tools.contains_key("tc-1"), "pending after start");
        }

        emit_flow_event(
            FlowStreamEvent::ToolCallDone {
                id: "tc-1".to_string(),
                result: Some(serde_json::json!("results")),
                error: None,
            },
            &emitter,
            "run-2",
            &state,
        )
        .await
        .expect("done ok");

        {
            let s = state.lock().await;
            assert!(!s.pending_tools.contains_key("tc-1"), "cleared after done");
        }

        let events = inner.events().await;

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], StreamEvent::ToolStart { .. }));
        assert!(matches!(events[1], StreamEvent::ToolEnd { .. }));
    }

    #[tokio::test]
    async fn safety_block_emits_error() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        emit_flow_event(
            FlowStreamEvent::SafetyBlock {
                reason: "blocked".to_string(),
            },
            &emitter,
            "run-3",
            &state,
        )
        .await
        .expect("safety block ok");

        let events = inner.events().await;

        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::RunError {
                error_code, error, ..
            } => {
                assert_eq!(error_code.as_deref(), Some("safety_block"));
                assert_eq!(error, "blocked");
            }
            other => panic!("expected RunError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn complete_returns_outcome() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        let outcome = FlowOutcome {
            final_text: "done".to_string(),
            iterations: 3,
            tool_calls_made: 2,
            total_tokens: 100,
            hit_limit: false,
        };

        emit_flow_event(
            FlowStreamEvent::Complete(outcome),
            &emitter,
            "run-4",
            &state,
        )
        .await
        .expect("complete ok");

        let events = inner.events().await;

        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::RunComplete {
                summary, run_id, ..
            } => {
                assert_eq!(run_id, "run-4");
                assert_eq!(summary.loops, 3);
                assert_eq!(summary.tool_calls, 2);
                assert_eq!(summary.final_response.as_deref(), Some("done"));
            }
            other => panic!("expected RunComplete, got {other:?}"),
        }
    }
}
