// core/ui/control_plane/src/views/halo/events.rs
//! Maps Gateway streaming events (run.*) to HaloState mutations.

use leptos::prelude::*;
use crate::context::{DashboardState, GatewayEvent};
use super::state::HaloState;

/// Subscribe to `run.*` events and dispatch to HaloState.
/// Returns the subscription ID for cleanup.
pub fn subscribe_run_events(dashboard: &DashboardState, halo: HaloState) -> usize {
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("run.") {
            return;
        }

        let data = &event.data;

        // Extract event type from the "type" field
        let event_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");

        // Guard: most events require a valid run_id to associate with a message
        if run_id.is_empty() && event_type != "reasoning" {
            return;
        }

        match event_type {
            "run_accepted" => {
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    halo.session_key.set(Some(sk.to_string()));
                }
                halo.start_assistant_message(run_id);
            }
            "reasoning" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    halo.reasoning_text.update(|t: &mut String| t.push_str(content));
                }
            }
            "tool_start" => {
                let name = data.get("tool_name").and_then(|n| n.as_str()).unwrap_or("tool");
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                halo.update_tool(run_id, tool_id, name, "running", None);
            }
            "tool_end" => {
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                let status = data.get("result")
                    .and_then(|r| r.get("success"))
                    .and_then(|s| s.as_bool())
                    .map(|ok| if ok { "completed" } else { "failed" })
                    .unwrap_or("completed");
                let duration = data.get("duration_ms").and_then(|d| d.as_u64());
                halo.update_tool(run_id, tool_id, "", status, duration);
            }
            "response_chunk" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    halo.append_chunk(run_id, content);
                }
            }
            "run_complete" => {
                halo.complete_run(run_id);
            }
            "run_error" => {
                let error = data.get("error").and_then(|e| e.as_str()).unwrap_or("Unknown error");
                halo.fail_run(run_id, error);
            }
            _ => {} // Ignore unknown event types
        }
    })
}
