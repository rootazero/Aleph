//! Maps Gateway streaming events (run.*) to ChatState mutations.

use super::state::{ChatState, ModelInfo};
use crate::context::{DashboardState, GatewayEvent};
use crate::state::layout::WorkspaceState;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
                let kind = trace_event
                    .get("kind")
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
            "run_complete" => {
                chat.complete_run(run_id);
                workspace.current_iteration.set(None);
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
            }
            _ => {} // Ignore unknown event types
        }
    })
}
