//! Maps Gateway streaming events (run.*) to ChatState mutations.

use super::state::{ChatState, ModelInfo};
use crate::context::{DashboardState, GatewayEvent};
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

/// Subscribe to `run.*` events and dispatch to ChatState.
/// Returns the subscription ID for cleanup.
pub fn subscribe_run_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    let trace_runs = Arc::new(Mutex::new(HashSet::<String>::new()));
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
                    }
                    "tool_summary" => {
                        if let Some(summary) = trace_event.get("summary").and_then(|s| s.as_str()) {
                            append_reasoning(chat, summary);
                        }
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
                let is_intermediate = data
                    .get("is_intermediate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Prefer "delta" field, fall back to "content" for backward compat
                let chunk_text = data
                    .get("delta")
                    .or_else(|| data.get("content"))
                    .and_then(|c| c.as_str());

                if is_intermediate {
                    if let Some(text) = chunk_text {
                        if !text.is_empty() {
                            chat.append_chunk(run_id, text);
                        }
                    }
                    chat.finalize_intermediate(run_id);
                } else if let Some(text) = chunk_text {
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
