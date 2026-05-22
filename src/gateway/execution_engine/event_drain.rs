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

use crate::gateway::event_emitter::{
    EventEmitError, EventEmitter, RunSummary, StreamEvent, ToolErrorItem, ToolSummaryItem,
};
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
use aleph_protocol::TokenBreakdownView;

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
///
/// P3a: populates the enriched `RunSummary` fields that were previously
/// always empty. Channels that ignore the new fields keep working — the
/// schema additions are all `#[serde(default)]`.
#[allow(dead_code)] // called from emit_flow_event which is itself dead_code until Task 4c
async fn emit_complete(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    outcome: &FlowOutcome,
) -> Result<(), EventEmitError> {
    let summary = build_run_summary(outcome);
    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq,
            summary,
            total_duration_ms: outcome.duration_ms,
        })
        .await
}

/// Build a wire-format [`RunSummary`] from a [`FlowOutcome`]. Extracted so
/// tests can assert mapping without spinning up an emitter.
pub(crate) fn build_run_summary(outcome: &FlowOutcome) -> RunSummary {
    let tool_summaries: Vec<ToolSummaryItem> = outcome
        .tool_timeline
        .iter()
        .map(|inv| ToolSummaryItem {
            tool_id: inv.id.clone(),
            tool_name: inv.name.clone(),
            // Emoji + display_meta stay in the renderer (P3b summary_format)
            // — leave neutral defaults here so channel-specific rendering
            // can override without re-walking the timeline.
            emoji: tool_emoji(&inv.name).to_string(),
            display_meta: String::new(),
            duration_ms: inv.duration_ms,
            success: inv.success,
        })
        .collect();

    let errors: Vec<ToolErrorItem> = outcome
        .tool_timeline
        .iter()
        .filter_map(|inv| {
            inv.error.as_ref().map(|err| ToolErrorItem {
                tool_name: inv.name.clone(),
                error: err.clone(),
                tool_id: inv.id.clone(),
            })
        })
        .collect();

    let token_breakdown =
        if outcome.token_breakdown == crate::orchestrator::dispatch::TokenBreakdown::default() {
            // No usage observed (test fixtures, providers that don't report) —
            // skip the field instead of serializing all-zeros.
            None
        } else {
            Some(TokenBreakdownView {
                input: outcome.token_breakdown.input,
                output: outcome.token_breakdown.output,
                cache_read: outcome.token_breakdown.cache_read,
                cache_creation: outcome.token_breakdown.cache_creation,
                reasoning: outcome.token_breakdown.reasoning,
            })
        };

    let (estimated_cost_usd, cost_status) = match outcome.estimated_cost.as_ref() {
        Some(c) => (
            Some(c.usd),
            Some(
                match c.status {
                    crate::pricing::CostStatus::Complete => "complete",
                    crate::pricing::CostStatus::PartialMissingPrice => "partial_missing_price",
                    crate::pricing::CostStatus::Unknown => "unknown",
                }
                .to_string(),
            ),
        ),
        None => (None, None),
    };

    RunSummary {
        total_tokens: u64::from(outcome.total_tokens),
        tool_calls: outcome.tool_calls_made,
        loops: outcome.iterations,
        final_response: if outcome.final_text.is_empty() {
            None
        } else {
            Some(outcome.final_text.clone())
        },
        duration_ms: if outcome.duration_ms == 0 {
            None
        } else {
            Some(outcome.duration_ms)
        },
        terminate_reason: Some(outcome.terminate_reason.as_static_str().to_string()),
        token_breakdown,
        estimated_cost_usd,
        cost_status,
        tool_summaries,
        errors,
    }
}

/// Per-tool emoji used by [`build_run_summary`] when constructing
/// [`ToolSummaryItem`]. Mirrors the hermes-agent display table for the
/// common tool names; falls back to a generic wrench for anything else.
fn tool_emoji(name: &str) -> &'static str {
    match name {
        "bash" | "shell" => "\u{26a1}",                             // ⚡
        "read" | "read_file" | "fs_read" => "\u{1f4d6}",            // 📖
        "write" | "write_file" | "fs_write" => "\u{270d}\u{fe0f}",  // ✍️
        "edit" | "patch" => "\u{270f}\u{fe0f}",                     // ✏️
        "web_search" | "search" | "google" => "\u{1f50d}",          // 🔍
        "browser" | "web_fetch" | "fetch" => "\u{1f310}",           // 🌐
        "memory_recall" | "memory_store" | "memory" => "\u{1f9e0}", // 🧠
        "spawn_subagent" | "subagent" => "\u{1f916}",               // 🤖
        _ => "\u{1f527}",                                           // 🔧
    }
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
            ..Default::default()
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

    /// P3a: rich `FlowOutcome` fields land in the enriched `RunSummary`
    /// fields. Verifies the timeline → tool_summaries mapping, terminate
    /// reason serialisation, token breakdown view, and error extraction
    /// from failed invocations.
    #[test]
    fn build_run_summary_carries_enriched_signals() {
        use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

        let outcome = FlowOutcome {
            final_text: "all done".into(),
            iterations: 4,
            tool_calls_made: 3,
            total_tokens: 1500,
            hit_limit: true,
            terminate_reason: TerminateReason::HitMaxIterations { used: 4 },
            duration_ms: 4321,
            token_breakdown: TokenBreakdown {
                input: 800,
                output: 600,
                cache_read: 50,
                cache_creation: 50,
                reasoning: 200,
            },
            tool_timeline: vec![
                ToolInvocation {
                    id: "t1".into(),
                    name: "bash".into(),
                    duration_ms: 100,
                    success: true,
                    error: None,
                },
                ToolInvocation {
                    id: "t2".into(),
                    name: "web_search".into(),
                    duration_ms: 250,
                    success: false,
                    error: Some("404 not found".into()),
                },
            ],
            estimated_cost: None,
        };

        let summary = super::build_run_summary(&outcome);

        // Legacy fields unchanged.
        assert_eq!(summary.total_tokens, 1500);
        assert_eq!(summary.tool_calls, 3);
        assert_eq!(summary.loops, 4);
        assert_eq!(summary.final_response.as_deref(), Some("all done"));

        // Enriched fields populated.
        assert_eq!(summary.duration_ms, Some(4321));
        assert_eq!(
            summary.terminate_reason.as_deref(),
            Some("hit_max_iterations")
        );
        let bd = summary.token_breakdown.expect("breakdown present");
        assert_eq!(bd.input, 800);
        assert_eq!(bd.output, 600);
        assert_eq!(bd.cache_read, 50);
        assert_eq!(bd.cache_creation, 50);
        assert_eq!(bd.reasoning, 200);

        // Tool timeline → tool_summaries.
        assert_eq!(summary.tool_summaries.len(), 2);
        assert_eq!(summary.tool_summaries[0].tool_name, "bash");
        assert_eq!(summary.tool_summaries[0].duration_ms, 100);
        assert!(summary.tool_summaries[0].success);
        // Emoji mapping fires for known tools.
        assert!(!summary.tool_summaries[0].emoji.is_empty());

        // Errors extracted only from failed invocations.
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(summary.errors[0].tool_name, "web_search");
        assert_eq!(summary.errors[0].error, "404 not found");

        // No cost configured.
        assert!(summary.estimated_cost_usd.is_none());
        assert!(summary.cost_status.is_none());
    }

    #[test]
    fn build_run_summary_with_zero_signals_is_minimal() {
        // A fresh outcome with no harness signals produces a summary that
        // matches the legacy shape (terminate_reason still populated since
        // FlowOutcome's default is TerminateReason::Completed).
        let outcome = FlowOutcome {
            final_text: "ok".into(),
            iterations: 1,
            tool_calls_made: 0,
            ..Default::default()
        };
        let summary = super::build_run_summary(&outcome);
        assert_eq!(summary.duration_ms, None, "zero duration → omitted");
        assert!(
            summary.token_breakdown.is_none(),
            "all-zero breakdown → omitted"
        );
        assert!(summary.tool_summaries.is_empty());
        assert!(summary.errors.is_empty());
        assert_eq!(summary.terminate_reason.as_deref(), Some("completed"));
    }
}
