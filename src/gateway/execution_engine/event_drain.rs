//! Event drain: maps `FlowStreamEvent` variants to `EventEmitter` calls.
//!
//! This is a pure, stateless-per-call helper extracted from the in-progress
//! `StreamCallback`. It holds drain-side state in `DrainState` (a single
//! `MessageAssembler` owning the visible-text accumulator, chunk counter, and
//! discard scrubber) so callers don't need to replicate that logic. Wired into
//! the live path by `helpers::run_dispatch_and_drain` (called from
//! `run_loop.rs`).

use crate::sync_primitives::Arc;

use tokio::sync::Mutex;

use crate::gateway::event_emitter::{
    EventEmitError, EventEmitter, RunSummary, StreamEvent, ToolErrorItem, ToolSummaryItem,
};
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent};
use aleph_protocol::TokenBreakdownView;

/// Mutable drain state shared across calls for a single run.
///
/// Owns the single [`MessageAssembler`](crate::gateway::message_assembly::MessageAssembler)
/// reducer for this run: the visible-text accumulator that populates each
/// `ResponseChunk.full_text`, the monotonic `chunk_index`, and the
/// cross-delta discard scrubber that strips echoed `<memory-context>` framing
/// AND inline reasoning/completion tags (`<think>` etc.) before they reach the
/// user. Consolidated from the retired `StreamingDeltaSink` + inline
/// `StreamingContextScrubber`. The drain task processes events serially, so
/// the `Mutex<DrainState>` is effectively uncontended.
#[derive(Debug, Default)]
pub(crate) struct DrainState {
    assembler: crate::gateway::message_assembly::MessageAssembler,
    /// `scratchpad` invocations seen this run: call id → "this call clears the
    /// plan". Populated on `ToolCallStart` because that is the only event
    /// carrying the tool name and its arguments.
    scratchpad_calls: std::collections::HashMap<String, bool>,
    /// Terminal execution list for this run, latched from the last
    /// `scratchpad` result. Stamped onto `RunSummary.plan` so renderers can
    /// reconcile a checklist whose live frames the lossy `agent_trace` WS
    /// mirror dropped — the same authoritative-terminal-state contract
    /// `tool_summaries` already provides for tool rows. `Some(empty)` after a
    /// `clear`, so the panel hides rather than freezing on the last checklist.
    plan: Option<aleph_protocol::plan::PlanSnapshot>,
}

/// Map one `FlowStreamEvent` to the appropriate `EventEmitter` call(s).
///
/// `run_id` is forwarded verbatim into every `StreamEvent` variant.
/// `state` accumulates cross-event bookkeeping (running text, chunk index).
///
/// This function is `async` because the `EventEmitter` trait is async.
pub(crate) async fn emit_flow_event(
    event: FlowStreamEvent,
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    state: &Arc<Mutex<DrainState>>,
) -> Result<(), EventEmitError> {
    match event {
        FlowStreamEvent::Delta(text) => {
            // Feed the raw delta through the run's assembler: strips echoed
            // `<memory-context>` framing AND inline reasoning/completion tags
            // across delta boundaries (G4), accumulates the running iteration
            // text (populates `full_text`), and stamps a monotonic chunk index.
            // A partial tag at the chunk tail is held inside the assembler and
            // surfaces on the next delta or the tool/run boundary flush.
            let emitted = {
                let mut s = state.lock().await;
                let visible = s.assembler.push_text_delta(&text);
                if visible.is_empty() {
                    None
                } else {
                    let full_text = s.assembler.snapshot().to_string();
                    let idx = s.assembler.next_chunk_index();
                    Some((visible, full_text, idx))
                }
            };
            if let Some((visible, full_text, idx)) = emitted {
                let seq = emitter.next_seq();
                emitter
                    .emit(StreamEvent::ResponseChunk {
                        run_id: run_id.to_string(),
                        seq,
                        delta: visible,
                        full_text,
                        chunk_index: idx,
                        is_final: false,
                        is_intermediate: false,
                    })
                    .await?;
            }
        }

        FlowStreamEvent::Reasoning(text) => {
            // Map to the existing Reasoning stream event. Forwarding to clients
            // is exercised by the gateway emitter (GatewayEventEmitter →
            // event bus → WebSocket / channel consumers).
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
            // Text-iteration boundary: drain any held-back scrubber tail and
            // reset the running `full_text` so the next iteration starts fresh.
            // Idempotent across parallel tool calls in one response (the first
            // clears the assembler's visible accumulator; later calls see it
            // already empty).
            flush_text_boundary(emitter, run_id, state).await?;
            {
                let mut s = state.lock().await;
                s.assembler.reset_iteration();
                if name == SCRATCHPAD_TOOL {
                    let clears =
                        args.get("action").and_then(serde_json::Value::as_str) == Some("clear");
                    s.scratchpad_calls.insert(id.clone(), clears);
                }
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

        FlowStreamEvent::ToolCallDone {
            id,
            result,
            error,
            duration_ms,
        } => {
            {
                // Latch the terminal execution list. This drain is in-process
                // and unbounded — unlike the WS `agent_trace` mirror it feeds —
                // so what we capture here is authoritative.
                let mut s = state.lock().await;
                // A failed call changed nothing on disk, so it must not move
                // the latch either — a rejected `clear` used to blank the strip.
                if let Some(clears) = s.scratchpad_calls.remove(&id) {
                    if error.is_none() {
                        if clears {
                            s.plan = Some(aleph_protocol::plan::PlanSnapshot::default());
                        } else if let Some(plan) = result.as_ref().and_then(extract_plan_snapshot) {
                            s.plan = Some(plan);
                        }
                    }
                }
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
                    duration_ms,
                })
                .await?;
        }

        FlowStreamEvent::ContextGauge {
            context_tokens,
            context_window,
            total_tokens,
        } => {
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::ContextGauge {
                    run_id: run_id.to_string(),
                    seq,
                    context_tokens,
                    context_window,
                    total_tokens,
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

        FlowStreamEvent::Complete(outcome) => {
            // Drain any partial-tag tail the scrubber held back at end of run
            // before the terminal summary, so a trailing non-fence fragment
            // isn't lost.
            flush_text_boundary(emitter, run_id, state).await?;
            let plan = state.lock().await.plan.clone();
            emit_complete(emitter, run_id, &outcome, plan).await?;
        }
    }
    Ok(())
}

/// Drain the scrubber's held-back partial-tag tail at a text-iteration or run
/// boundary. If the tail turned out not to be a real fence it is emitted as a
/// trailing chunk; a genuine unterminated span is discarded (never leaked).
/// `MessageAssembler::flush_boundary` resets the scrubber either way, so the
/// next iteration starts clean. No-op when the scrubber holds nothing.
async fn flush_text_boundary(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    state: &Arc<Mutex<DrainState>>,
) -> Result<(), EventEmitError> {
    let emitted = {
        let mut s = state.lock().await;
        // Drain any held-back partial-tag tail; `flush_boundary` appends it to
        // the assembler's visible accumulator, so `snapshot()` includes it.
        let tail = s.assembler.flush_boundary();
        if tail.is_empty() {
            None
        } else {
            let full_text = s.assembler.snapshot().to_string();
            let idx = s.assembler.next_chunk_index();
            Some((tail, full_text, idx))
        }
    };
    if let Some((tail, full_text, idx)) = emitted {
        let seq = emitter.next_seq();
        emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq,
                delta: tail,
                full_text,
                chunk_index: idx,
                is_final: false,
                is_intermediate: false,
            })
            .await?;
    }
    Ok(())
}

/// Helper: emit the terminal `RunComplete` event from a `FlowOutcome`.
///
/// P3a: populates the enriched `RunSummary` fields that were previously
/// always empty. Channels that ignore the new fields keep working — the
/// schema additions are all `#[serde(default)]`.
async fn emit_complete(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    outcome: &FlowOutcome,
    plan: Option<aleph_protocol::plan::PlanSnapshot>,
) -> Result<(), EventEmitError> {
    let summary = build_run_summary(outcome, plan);
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
pub(crate) fn build_run_summary(
    outcome: &FlowOutcome,
    plan: Option<aleph_protocol::plan::PlanSnapshot>,
) -> RunSummary {
    let tool_summaries: Vec<ToolSummaryItem> = outcome
        .tool_timeline
        .iter()
        .map(|inv| ToolSummaryItem {
            tool_id: inv.id.clone(),
            tool_name: inv.name.clone(),
            // Per-tool emoji resolved here once; renderers (runtime footer,
            // Panel) consume it as-is instead of re-walking the timeline.
            emoji: tool_emoji(&inv.name).to_string(),
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

    // The umbrella `BudgetExhaustedPartialResult` variant collapses to
    // the static token `"budget_exhausted_partial_result"`. Surface the
    // inner cap label separately so cron carry-over + dashboards can
    // show which budget actually fired without re-deserialising the enum.
    let terminate_detail = match &outcome.terminate_reason {
        crate::orchestrator::dispatch::TerminateReason::BudgetExhaustedPartialResult {
            reason,
            ..
        } => Some(reason.clone()),
        _ => None,
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
        terminate_detail,
        token_breakdown,
        context_tokens: outcome.context_tokens,
        context_window: outcome.context_window,
        estimated_cost_usd,
        cost_status,
        tool_summaries,
        errors,
        plan,
    }
}

/// Tool whose result carries the execution list.
const SCRATCHPAD_TOOL: &str = "scratchpad";

/// Read the `snapshot` the `scratchpad` tool attaches to its output.
///
/// One known shape — the raw `ScratchpadOutput` JSON that `act.rs` hands to
/// `on_tool_call_done` — deliberately enumerated rather than searched for at
/// any depth, so a future tool that happens to have a `snapshot` field cannot
/// silently hijack the plan slot.
fn extract_plan_snapshot(result: &serde_json::Value) -> Option<aleph_protocol::plan::PlanSnapshot> {
    let raw = result.get("snapshot")?;
    serde_json::from_value(raw.clone())
        .inspect_err(|e| {
            tracing::warn!(error = %e, ?raw, "scratchpad snapshot deserialization failed");
        })
        .ok()
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
    }

    /// Concatenate the visible content of every emitted ResponseChunk.
    fn emitted_text(events: &[StreamEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ResponseChunk { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    /// full_text accumulates across deltas within an iteration and chunk_index
    /// is monotonic — the live path now populates both (previously always
    /// `""` / `0`).
    #[tokio::test]
    async fn full_text_accumulates_and_chunk_index_is_monotonic() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        for t in ["Hello", ", ", "world"] {
            emit_flow_event(
                FlowStreamEvent::Delta(t.to_string()),
                &emitter,
                "run-acc",
                &state,
            )
            .await
            .expect("emit ok");
        }

        let events = inner.events().await;
        let chunks: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ResponseChunk {
                    full_text,
                    chunk_index,
                    ..
                } => Some((full_text.clone(), *chunk_index)),
                _ => None,
            })
            .collect();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], ("Hello".to_string(), 0));
        assert_eq!(chunks[1], ("Hello, ".to_string(), 1));
        assert_eq!(chunks[2], ("Hello, world".to_string(), 2));
    }

    /// A model that echoes the injected `<memory-context>` framing must have
    /// the fenced span stripped before it reaches the user — the security
    /// guarantee migrated from the retired StreamingDeltaSink onto the live
    /// drain path.
    #[tokio::test]
    async fn memory_context_echo_is_scrubbed_on_live_path() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        let echo = "Sure. <memory-context>\nSECRET RECALL\n</memory-context> Done.";
        emit_flow_event(
            FlowStreamEvent::Delta(echo.to_string()),
            &emitter,
            "run-fence",
            &state,
        )
        .await
        .expect("emit ok");
        emit_flow_event(
            FlowStreamEvent::Complete(FlowOutcome::default()),
            &emitter,
            "run-fence",
            &state,
        )
        .await
        .expect("complete ok");

        let text = emitted_text(&inner.events().await);
        assert!(
            !text.contains("SECRET RECALL"),
            "fenced body must not leak: {text:?}"
        );
        assert!(!text.contains("memory-context"), "fence tag must not leak");
        assert!(text.contains("Sure."));
        assert!(text.contains("Done."));
    }

    /// The fence may be split across deltas (open in one, close in a later one);
    /// the scrubber's cross-delta state machine must still strip the whole span.
    #[tokio::test]
    async fn split_fence_across_deltas_is_scrubbed() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        for t in ["answer <memory-", "context>leaky</memory-", "context> tail"] {
            emit_flow_event(
                FlowStreamEvent::Delta(t.to_string()),
                &emitter,
                "run-split",
                &state,
            )
            .await
            .expect("emit ok");
        }
        emit_flow_event(
            FlowStreamEvent::Complete(FlowOutcome::default()),
            &emitter,
            "run-split",
            &state,
        )
        .await
        .expect("complete ok");

        let text = emitted_text(&inner.events().await);
        assert!(
            !text.contains("leaky"),
            "split fence body must not leak: {text:?}"
        );
        assert!(text.contains("answer"));
        assert!(text.contains("tail"));
    }

    /// A tool boundary resets the running `full_text` so the next iteration's
    /// accumulated text starts fresh (chunk_index stays monotonic run-wide).
    #[tokio::test]
    async fn tool_boundary_resets_full_text() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        emit_flow_event(
            FlowStreamEvent::Delta("First part.".to_string()),
            &emitter,
            "run-iter",
            &state,
        )
        .await
        .expect("emit ok");
        emit_flow_event(
            FlowStreamEvent::ToolCallStart {
                id: "tc-1".to_string(),
                name: "search".to_string(),
                args: serde_json::json!({}),
            },
            &emitter,
            "run-iter",
            &state,
        )
        .await
        .expect("tool start ok");
        emit_flow_event(
            FlowStreamEvent::Delta("Second part.".to_string()),
            &emitter,
            "run-iter",
            &state,
        )
        .await
        .expect("emit ok");

        let events = inner.events().await;
        let last_full = events
            .iter()
            .rev()
            .find_map(|e| match e {
                StreamEvent::ResponseChunk { full_text, .. } => Some(full_text.clone()),
                _ => None,
            })
            .expect("a response chunk exists");
        assert_eq!(
            last_full, "Second part.",
            "second iteration full_text must not carry the first"
        );
    }

    /// Inline `<think>` blocks — even split across delta boundaries — are
    /// stripped from the live `ResponseChunk` stream (G4). Previously the drain
    /// only scrubbed `<memory-context>`, so raw reasoning leaked live and was
    /// only retro-cleaned at RunComplete.
    #[tokio::test]
    async fn inline_think_is_stripped_from_live_response_chunks() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        for d in ["Here is <thi", "nk>hidden</think> the answer"] {
            emit_flow_event(
                FlowStreamEvent::Delta(d.to_string()),
                &emitter,
                "r1",
                &state,
            )
            .await
            .expect("emit ok");
        }

        let joined: String = inner
            .events()
            .await
            .into_iter()
            .filter_map(|e| match e {
                StreamEvent::ResponseChunk { delta, .. } => Some(delta),
                _ => None,
            })
            .collect();
        assert_eq!(
            joined, "Here is  the answer",
            "no raw <think> in the live stream"
        );
    }

    /// Drive one scratchpad call through the drain and return the run's
    /// latched terminal plan.
    async fn drain_scratchpad(
        action: &str,
        result: serde_json::Value,
    ) -> Option<aleph_protocol::plan::PlanSnapshot> {
        let (_inner, emitter) = make_emitter();
        let state = make_state();
        emit_flow_event(
            FlowStreamEvent::ToolCallStart {
                id: "sp-1".to_string(),
                name: "scratchpad".to_string(),
                args: serde_json::json!({ "action": action }),
            },
            &emitter,
            "run-plan",
            &state,
        )
        .await
        .expect("start ok");
        emit_flow_event(
            FlowStreamEvent::ToolCallDone {
                id: "sp-1".to_string(),
                result: Some(result),
                error: None,
                duration_ms: 3,
            },
            &emitter,
            "run-plan",
            &state,
        )
        .await
        .expect("done ok");
        let plan = state.lock().await.plan.clone();
        plan
    }

    #[tokio::test]
    async fn scratchpad_result_latches_the_terminal_plan_onto_the_summary() {
        let plan = drain_scratchpad(
            "complete_item",
            serde_json::json!({
                "success": true,
                "message": "ok",
                "snapshot": {
                    "objective": "Ship auth",
                    "complete": false,
                    "items": [
                        {"text": "Design", "status": "completed"},
                        {"text": "Build", "status": "in_progress"},
                    ]
                }
            }),
        )
        .await
        .expect("a latched plan");
        assert_eq!(plan.done_count(), 1);
        assert_eq!(plan.current_step(), Some("Build"));

        let summary = super::build_run_summary(&FlowOutcome::default(), Some(plan));
        let carried = summary.plan.expect("summary carries the plan");
        assert_eq!(
            carried.total(),
            2,
            "run_complete is the authoritative source"
        );
    }

    #[tokio::test]
    async fn a_clear_latches_an_empty_plan_rather_than_the_stale_one() {
        // Without this the panel would freeze on the last checklist forever:
        // `clear` returns no snapshot, so "keep the last one" is wrong.
        let plan = drain_scratchpad("clear", serde_json::json!({"success": true}))
            .await
            .expect("an explicit empty plan");
        assert!(!plan.has_content());
    }

    #[tokio::test]
    async fn a_failed_scratchpad_call_does_not_move_the_latch() {
        let (_inner, emitter) = make_emitter();
        let state = make_state();
        emit_flow_event(
            FlowStreamEvent::ToolCallStart {
                id: "sp-1".to_string(),
                name: "scratchpad".to_string(),
                args: serde_json::json!({ "action": "clear" }),
            },
            &emitter,
            "run-err",
            &state,
        )
        .await
        .unwrap();
        emit_flow_event(
            FlowStreamEvent::ToolCallDone {
                id: "sp-1".to_string(),
                result: None,
                error: Some("permission denied".to_string()),
                duration_ms: 1,
            },
            &emitter,
            "run-err",
            &state,
        )
        .await
        .unwrap();
        assert!(
            state.lock().await.plan.is_none(),
            "a rejected clear changed nothing on disk, so it must not blank the strip"
        );
    }

    #[tokio::test]
    async fn a_non_scratchpad_tool_never_writes_the_plan_slot() {
        let (_inner, emitter) = make_emitter();
        let state = make_state();
        emit_flow_event(
            FlowStreamEvent::ToolCallStart {
                id: "t-1".to_string(),
                name: "note_manage".to_string(),
                args: serde_json::json!({ "action": "clear" }),
            },
            &emitter,
            "run-x",
            &state,
        )
        .await
        .unwrap();
        emit_flow_event(
            FlowStreamEvent::ToolCallDone {
                id: "t-1".to_string(),
                result: Some(serde_json::json!({
                    "snapshot": {"objective": "hijack", "items": [], "complete": false}
                })),
                error: None,
                duration_ms: 1,
            },
            &emitter,
            "run-x",
            &state,
        )
        .await
        .unwrap();
        assert!(state.lock().await.plan.is_none());
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

        emit_flow_event(
            FlowStreamEvent::ToolCallDone {
                id: "tc-1".to_string(),
                result: Some(serde_json::json!("results")),
                error: None,
                duration_ms: 137,
            },
            &emitter,
            "run-2",
            &state,
        )
        .await
        .expect("done ok");

        let events = inner.events().await;

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], StreamEvent::ToolStart { .. }));
        // The harness-measured elapsed time reaches the live tool card
        // (previously hardcoded to 0 here, only available at RunComplete).
        match &events[1] {
            StreamEvent::ToolEnd { duration_ms, .. } => assert_eq!(*duration_ms, 137),
            other => panic!("expected ToolEnd, got {other:?}"),
        }
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
            context_tokens: 1234,
            context_window: 200_000,
            serving_model: None,
            serving_provider: None,
        };

        let summary = super::build_run_summary(&outcome, None);

        assert_eq!(summary.context_tokens, 1234);
        assert_eq!(summary.context_window, 200_000);

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
        let summary = super::build_run_summary(&outcome, None);
        assert_eq!(summary.duration_ms, None, "zero duration → omitted");
        assert!(
            summary.token_breakdown.is_none(),
            "all-zero breakdown → omitted"
        );
        assert!(summary.tool_summaries.is_empty());
        assert!(summary.errors.is_empty());
        assert_eq!(summary.terminate_reason.as_deref(), Some("completed"));
        // The granular detail field is `None` for every non-umbrella
        // terminate reason (here: clean Completed).
        assert_eq!(
            summary.terminate_detail, None,
            "terminate_detail is None unless variant is BudgetExhaustedPartialResult",
        );
    }

    /// Granular cap label exposed via `terminate_detail` when the
    /// harness escalates a bare budget cap to
    /// `BudgetExhaustedPartialResult`. The outer `terminate_reason`
    /// keeps the umbrella label for dashboard back-compat; the inner
    /// `terminate_detail` carries the underlying cap so cron carry-over
    /// can record *which* budget fired.
    #[test]
    fn build_run_summary_exposes_granular_detail_for_partial_result() {
        use crate::orchestrator::dispatch::TerminateReason;

        let outcome = FlowOutcome {
            final_text: "I gathered 2 of 5 sources".into(),
            iterations: 40,
            terminate_reason: TerminateReason::BudgetExhaustedPartialResult {
                reason: "hit_max_iterations".into(),
                partial_summary: "I gathered 2 of 5 sources".into(),
            },
            ..Default::default()
        };
        let summary = super::build_run_summary(&outcome, None);
        assert_eq!(
            summary.terminate_reason.as_deref(),
            Some("budget_exhausted_partial_result"),
            "umbrella label preserved for dashboards",
        );
        assert_eq!(
            summary.terminate_detail.as_deref(),
            Some("hit_max_iterations"),
            "granular cap label exposed for cron carry-over",
        );
    }

    /// Non-budget terminate reasons leave `terminate_detail` empty —
    /// the field exists solely to unwrap the umbrella variant.
    #[test]
    fn build_run_summary_omits_detail_for_non_budget_reasons() {
        use crate::orchestrator::dispatch::TerminateReason;

        let cases = [
            TerminateReason::Completed,
            TerminateReason::Cancelled,
            TerminateReason::HitMaxIterations { used: 5 },
            TerminateReason::ContextBudgetExhausted,
            TerminateReason::MaxOutputTokensExhausted,
            TerminateReason::StallTimeout { elapsed_ms: 100 },
            TerminateReason::VerifierVeto { vetos: 3 },
        ];
        for reason in cases {
            let label = reason.as_static_str().to_string();
            let outcome = FlowOutcome {
                terminate_reason: reason,
                ..Default::default()
            };
            let summary = super::build_run_summary(&outcome, None);
            assert_eq!(
                summary.terminate_detail, None,
                "terminate_detail must be None for {label}",
            );
        }
    }
}
