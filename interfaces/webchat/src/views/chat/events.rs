//! Maps Gateway streaming events (run.*) to ChatState mutations.

use leptos::prelude::*;
use crate::context::{DashboardState, GatewayEvent};
use super::state::{ChatState, ModelInfo};

/// Subscribe to `run.*` events and dispatch to ChatState.
/// Returns the subscription ID for cleanup.
pub fn subscribe_run_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("run.") {
            return;
        }

        let data = &event.data;

        // Extract event type: prefer data.type, fallback to topic suffix (e.g. "run.run_accepted" -> "run_accepted")
        let event_type = data.get("type").and_then(|t| t.as_str())
            .or_else(|| event.topic.strip_prefix("run."))
            .unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");

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
                    chat.reasoning_text.update(|t: &mut String| t.push_str(content));
                }
            }
            "tool_start" => {
                let name = data.get("tool_name").and_then(|n| n.as_str()).unwrap_or("tool");
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                chat.update_tool(run_id, tool_id, name, "running", None);
            }
            "tool_end" => {
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                let status = data.get("result")
                    .and_then(|r| r.get("success"))
                    .and_then(|s| s.as_bool())
                    .map(|ok| if ok { "completed" } else { "failed" })
                    .unwrap_or("completed");
                let duration = data.get("duration_ms").and_then(|d| d.as_u64());
                chat.update_tool(run_id, tool_id, "", status, duration);
            }
            "response_chunk" => {
                let is_intermediate = data.get("is_intermediate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Prefer "delta" field, fall back to "content" for backward compat
                let chunk_text = data.get("delta")
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
                let model = data.get("model_info").and_then(|m| {
                    serde_json::from_value::<ModelInfo>(m.clone()).ok()
                });
                if let Some(info) = model {
                    chat.set_model_info(run_id, info);
                }
            }
            "run_complete" => {
                chat.complete_run(run_id);
            }
            "run_error" => {
                let error = data.get("error").and_then(|e| e.as_str()).unwrap_or("Unknown error");
                chat.fail_run(run_id, error);
            }
            _ => {} // Ignore unknown event types
        }
    })
}
