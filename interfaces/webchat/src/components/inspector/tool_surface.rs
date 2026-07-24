//! Tool inspector surface — the original `ToolDetailView` body, unchanged.
//!
//! Shows one tool call's full (uncapped) args/result/diff via the SAME
//! `tool_card::render_body` the inline chat card uses, but with
//! `ToolSurface::Detail` (no 8-line cap). Reactive on `chat.messages`
//! (name/status/duration) and `workspace.tool_payloads` (args/result), so it
//! keeps updating while a followed tool streams to completion.

use crate::state::layout::WorkspaceState;
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Detail inspector — right-pane tool surface: full args/result/diff for the currently selected tool (uncapped).
#[component]
pub fn ToolInspector(run_id: String, tool_id: String) -> impl IntoView {
    use crate::components::tool_card::{
        render_body, tool_headline, tool_icon, ToolKind, ToolSurface,
    };
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();

    // Reactive: the followed tool's status/duration land after selection, and
    // its result payload arrives on `tool_call_completed`.
    move || {
        // Name/status reverse-looked up from transcript; payload fetched from capture table.
        let entry = chat.messages.with(|msgs| {
            msgs.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == tool_id)
                .cloned()
        });
        let tool_name = entry
            .as_ref()
            .map(|t| t.tool_name.clone())
            .unwrap_or_default();
        let status = entry.as_ref().map(|t| t.status.clone()).unwrap_or_default();
        let duration = entry.as_ref().and_then(|t| t.duration_ms);
        let kind = ToolKind::from_name(&tool_name);
        let payload = workspace.get_tool_payload(&run_id, &tool_id);
        let headline = tool_headline(kind, &payload).unwrap_or_else(|| tool_name.clone());
        let icon = tool_icon(&tool_name, kind);
        let status_view = match status.as_str() {
            "running" => view! {
                <span class="inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
            }
            .into_any(),
            "failed" => view! { <span class="text-danger text-xs">"✗"</span> }.into_any(),
            _ => view! { <span class="text-success text-xs">"✓"</span> }.into_any(),
        };
        view! {
            <div class="flex flex-col gap-2">
                <div class="flex items-center gap-2 pb-2 border-b border-border/60">
                    <span class="text-base shrink-0">{icon}</span>
                    <span class="flex-1 min-w-0 truncate text-sm text-text-primary font-medium">
                        {headline}
                    </span>
                    {status_view}
                    {duration.map(|d| view! {
                        <span class="text-[10px] font-mono text-text-tertiary">
                            {crate::state::run_clock::fmt_elapsed(d as i64)}
                        </span>
                    })}
                </div>
                {render_body(kind, &payload, ToolSurface::Detail, String::new(), || {})}
            </div>
        }
        .into_any()
    }
}
