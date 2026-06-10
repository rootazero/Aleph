//! Maps Gateway streaming events (run.*) to ChatState mutations.

use super::state::{ChatState, ContextUsage, ModelInfo, ProviderRetryNotice};
use crate::context::{DashboardState, GatewayEvent};
use crate::state::layout::WorkspaceState;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Project one persisted/live `AgentTraceEvent` (tagged `kind` or `type`)
/// onto ChatState (step bubbles, tool status, narration) + WorkspaceState
/// (tool args/result payloads, current iteration). The single projection
/// shared by the live WS stream and `trace.by_runs` replay so the two paths
/// can never drift.
pub(crate) fn apply_trace_event(
    chat: ChatState,
    workspace: WorkspaceState,
    run_id: &str,
    trace_event: &serde_json::Value,
) {
    // The harness serializes `LoopTraceEvent` with `#[serde(tag =
    // "type")]`, so the discriminator arrives as `type`. The
    // protocol `AgentTraceEvent` form tags it `kind`; accept either
    // so both wire shapes parse.
    let kind = trace_event
        .get("type")
        .or_else(|| trace_event.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");

    match kind {
        "tool_call_started" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("tool");
            chat.update_tool(run_id, tool_id, tool_name, "running", None);
            // Surface activity on the toggle when the pane is
            // closed (R5 — never force-open the Split).
            workspace.note_activity();
            // Capture args/input for the workspace pane. Schema
            // varies by tool kind — try the two known keys.
            if !tool_id.is_empty() {
                let args = call
                    .and_then(|c| c.get("input").or_else(|| c.get("args")))
                    .cloned();
                if let Some(args) = args {
                    workspace.record_tool_args(run_id, tool_id, args);
                }
            }
        }
        "tool_call_completed" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let duration = call
                .and_then(|c| c.get("duration_ms"))
                .and_then(|d| d.as_u64());
            let result = trace_event
                .get("result")
                .unwrap_or(&serde_json::Value::Null);
            let status = if result.get("Error").is_some() {
                "failed"
            } else {
                "completed"
            };
            chat.update_tool(run_id, tool_id, tool_name, status, duration);
            // Mirror the result into workspace state so the
            // tool-detail view can show actual output.
            if !tool_id.is_empty() {
                workspace.record_tool_result(run_id, tool_id, result.clone());
            }
        }
        "tool_summary" => {
            if let Some(summary) = trace_event.get("summary").and_then(|s| s.as_str()) {
                append_reasoning(chat, summary);
            }
        }
        "turn_started" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                return;
            };
            chat.begin_step(run_id, iteration);
            workspace.set_current_iteration(run_id, iteration);
        }
        "text_emitted" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                return;
            };
            let text = trace_event
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            chat.set_step_text(run_id, iteration, text);
        }
        _ => {}
    }
}

/// Reconstruct one assistant run's chat bubbles + workspace tool payloads from
/// its persisted trace `events`, mirroring the live event order: open the
/// `assistant-{run}` placeholder, project every event through
/// `apply_trace_event`, finalize the turn, then overwrite the trailing answer
/// bubble's text with the history-authoritative `final_content`. Earlier turns
/// become `intermediate-{run}-{n}` bubbles; the trailing `assistant-{run}`
/// bubble is the final answer.
pub(crate) fn replay_run(
    chat: ChatState,
    workspace: WorkspaceState,
    run_id: &str,
    events: &[serde_json::Value],
    final_content: &str,
) {
    chat.start_assistant_message(run_id);
    for ev in events {
        apply_trace_event(chat, workspace, run_id, ev);
    }
    chat.complete_run(run_id);
    // Same authoritative promotion as the live `run_complete` path: overwrite
    // the trailing bubble with the history-authoritative answer and flag it
    // `is_final` so a turn that ended with text + a tool call still renders the
    // answer as a bubble, not a trapped step.
    chat.finalize_answer(run_id, final_content);
}

fn append_reasoning(chat: ChatState, summary: &str) {
    if summary.is_empty() {
        return;
    }

    chat.reasoning_text.update(|text: &mut String| {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(summary);
    });
}

/// Subscribe to `run.*` events and dispatch to ChatState. Tool args/results
/// are mirrored into [`WorkspaceState::tool_payloads`] so the workspace
/// pane can render real invocation details without an extra round-trip.
/// Returns the subscription ID for cleanup.
pub fn subscribe_run_events(
    dashboard: &DashboardState,
    chat: ChatState,
    workspace: WorkspaceState,
) -> usize {
    let trace_runs = Arc::new(Mutex::new(HashSet::<String>::new()));
    // Owned Copy captured into the 'static event closure — used to drive
    // voice-loop TTS playback when a registered run completes.
    let dash = *dashboard;
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("run.") {
            return;
        }

        let data = &event.data;
        let trace_runs = trace_runs.clone();

        // Extract event type: prefer data.type, fallback to topic suffix (e.g. "run.run_accepted" -> "run_accepted")
        let event_type = data
            .get("type")
            .and_then(|t| t.as_str())
            .or_else(|| event.topic.strip_prefix("run."))
            .unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");
        let trace_enabled = !run_id.is_empty()
            && trace_runs
                .lock()
                .map(|runs| runs.contains(run_id))
                .unwrap_or(false);

        // Guard: most events require a valid run_id to associate with a message
        if run_id.is_empty() && event_type != "reasoning" {
            return;
        }

        match event_type {
            "run_accepted" => {
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    chat.session_key.set(Some(sk.to_string()));
                }
                chat.start_assistant_message(run_id);
            }
            "reasoning" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    chat.reasoning_text
                        .update(|t: &mut String| t.push_str(content));
                }
            }
            "agent_trace" => {
                if let Ok(mut runs) = trace_runs.lock() {
                    runs.insert(run_id.to_string());
                }
                let Some(trace_event) = data.get("event") else {
                    return;
                };
                apply_trace_event(chat, workspace, run_id, trace_event);
            }
            "tool_start" => {
                if trace_enabled {
                    return;
                }
                let name = data
                    .get("tool_name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool");
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                chat.update_tool(run_id, tool_id, name, "running", None);
            }
            "tool_end" => {
                if trace_enabled {
                    return;
                }
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                let status = data
                    .get("result")
                    .and_then(|r| r.get("success"))
                    .and_then(|s| s.as_bool())
                    .map(|ok| if ok { "completed" } else { "failed" })
                    .unwrap_or("completed");
                let duration = data.get("duration_ms").and_then(|d| d.as_u64());
                chat.update_tool(run_id, tool_id, "", status, duration);
            }
            "response_chunk" => {
                // Live typewriter preview. When trace is active, authoritative
                // per-step text arrives via `agent_trace.text_emitted` (both
                // output modes) and overwrites this preview, so the `is_final`
                // chunk — in instant mode the whole-run buffered dump — is
                // dropped to avoid duplicating already-set step text. For
                // non-trace runs no text_emitted arrives, so the `is_final`
                // chunk is the only text source and must be kept.
                let is_final = data
                    .get("is_final")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_final && trace_enabled {
                    return;
                }
                let chunk_text = data
                    .get("delta")
                    .or_else(|| data.get("content"))
                    .and_then(|c| c.as_str());
                if let Some(text) = chunk_text {
                    chat.append_chunk(run_id, text);
                }
            }
            "model_resolved" => {
                let model = data
                    .get("model_info")
                    .and_then(|m| serde_json::from_value::<ModelInfo>(m.clone()).ok());
                if let Some(info) = model {
                    chat.set_model_info(run_id, info);
                }
            }
            "run_retrying" => {
                // Provider chain failed transiently; surface the retry under
                // the thinking indicator instead of leaving minutes of
                // silence. Cleared on the next chunk / run settle.
                let provider = data
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let attempt = data.get("attempt").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let max_attempts = data
                    .get("max_attempts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                chat.set_provider_retry(ProviderRetryNotice {
                    provider,
                    attempt,
                    max_attempts,
                });
            }
            "run_complete" => {
                chat.complete_run(run_id);
                workspace.current_iteration.set(None);
                // Promote the harness-authoritative final answer into the
                // trailing bubble so it renders as the conversational reply —
                // even when the terminating turn also issued a tool call (which
                // would otherwise keep `is_final_answer` false and trap the
                // answer in the step strip). Mirrors `replay_run`'s overwrite.
                if let Some(final_text) = data
                    .get("summary")
                    .and_then(|s| s.get("final_response"))
                    .and_then(|v| v.as_str())
                {
                    chat.finalize_answer(run_id, final_text);
                }
                // Context gauge: the run summary already ships token_breakdown
                // (input = last-turn context tokens) + total_tokens. Resolve the
                // window denominator from the run's model client-side and publish
                // for the composer's ContextGauge. Additive read of data already
                // on the wire — no backend protocol change.
                if let Some(summary) = data.get("summary") {
                    let input = summary
                        .get("token_breakdown")
                        .and_then(|b| b.get("input"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let total = summary
                        .get("total_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if input > 0 || total > 0 {
                        let model = chat.model_for_run(run_id).unwrap_or_default();
                        let window = super::context_gauge::context_window_for(&model);
                        chat.context_usage.set(Some(ContextUsage {
                            used_tokens: input,
                            window_tokens: window,
                            total_tokens: total,
                        }));
                    }
                }
                // Voice loop: if the mic button registered this run, speak the
                // final reply via the core TTS path → endpoint playback.
                if chat.take_speak_run(run_id) {
                    let text = chat.assistant_text_for_run(run_id);
                    if !text.trim().is_empty() {
                        super::voice_playback::speak(&dash, text);
                    }
                }
            }
            "run_error" => {
                let error = data
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Unknown error");
                chat.fail_run(run_id, error);
                workspace.current_iteration.set(None);
            }
            _ => {} // Ignore unknown event types
        }
    })
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    use crate::state::layout::WorkspaceState;
    use crate::views::chat::state::ChatState;
    use leptos::prelude::Owner;
    use serde_json::json;

    #[test]
    fn replay_run_rebuilds_intermediates_then_final_answer() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();

        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "text_emitted", "iteration": 1, "stream": "step", "text": "looking" }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "input": { "q": "x" } } }),
            json!({ "kind": "tool_call_completed", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "duration_ms": 3 },
                    "result": { "ok": true } }),
            json!({ "kind": "turn_started", "iteration": 2 }),
            json!({ "kind": "text_emitted", "iteration": 2, "stream": "final", "text": "raw final" }),
        ];

        replay_run(chat, ws, "run-1", &events, "AUTHORITATIVE ANSWER");

        let msgs = chat.messages.get_untracked();
        assert!(
            msgs.iter()
                .any(|m| m.is_intermediate && m.id.starts_with("intermediate-run-1-")),
            "expected an intermediate step bubble"
        );
        let final_bubble = msgs
            .iter()
            .find(|m| m.id == "assistant-run-1")
            .expect("final answer bubble");
        assert!(!final_bubble.is_intermediate);
        assert!(!final_bubble.is_streaming);
        assert_eq!(final_bubble.content, "AUTHORITATIVE ANSWER");
        assert!(ws.get_tool_payload("run-1", "t1").is_some());
    }

    #[test]
    fn apply_trace_event_builds_steps_and_payloads() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();

        // Simulate run_accepted which creates the placeholder message bubble
        // that begin_step / set_step_text require to exist.
        chat.start_assistant_message("run-1");

        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "input": { "q": "rust" } } }),
            json!({ "kind": "tool_call_completed", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "duration_ms": 5 },
                    "result": { "ok": true } }),
            json!({ "kind": "turn_started", "iteration": 2 }),
            json!({ "kind": "text_emitted", "iteration": 2, "stream": "final", "text": "done" }),
        ];
        for ev in &events {
            apply_trace_event(chat, ws, "run-1", ev);
        }

        let payload = ws.get_tool_payload("run-1", "t1").expect("payload");
        assert!(payload.args.is_some());
        assert!(payload.result.is_some());

        let msgs = chat.messages.get_untracked();
        let tagged: Vec<usize> = msgs.iter().filter_map(|m| m.iteration).collect();
        assert_eq!(tagged, vec![1, 2]);
        assert!(msgs
            .iter()
            .any(|m| m.iteration == Some(2) && m.content.contains("done")));
    }
}
