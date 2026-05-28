//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! Decides what to render based on [`WorkspaceContent`]:
//!
//! - `Empty`     — hero placeholder
//! - `ToolDetail` — looks the tool entry up in `ChatState.messages` and
//!                  dispatches it through the [`ToolRendererRegistry`]
//! - `Notes`     — read-only markdown preview of the freeform scratchpad
//!
//! Mount is conditional in `app.rs`: only when chat mode is active AND
//! `LayoutMode::Split` is set, the panel slides in as a flex sibling of
//! the chat surface.

use crate::components::json_viewer::JsonViewer;
use crate::components::markdown::MarkdownRenderer;
use crate::components::tool_renderer::ToolRendererRegistry;
use crate::i18n::*;
use crate::state::layout::{LayoutMode, ToolPayload, WorkspaceContent, WorkspaceState};
use crate::views::chat::state::{ChatState, ToolCallEntry};
use leptos::prelude::*;

/// Workspace pane root. Renders nothing when [`LayoutMode::ChatOnly`].
#[component]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();

    view! {
        <Show when=move || workspace.mode.get() == LayoutMode::Split>
            <aside class="aleph-workspace-pane flex flex-col h-full
                           border-l border-border bg-surface-base/40
                           min-w-[280px] basis-[66%] shrink overflow-hidden">
                <WorkspaceHeader />
                <div class="flex-1 overflow-y-auto px-4 py-3">
                    <WorkspaceBody />
                </div>
            </aside>
        </Show>
    }
}

/// Sticky header — text-only title chip ("WORKSPACE · idle"). No border,
/// no icon, no close button:
/// * the divider line was removed to mirror the chat surface (which has
///   no top bar) — the macOS titlebar drag strip covers this row anyway,
///   so the area still drags the window;
/// * the leading SVG was decorative-only and added visual noise without
///   carrying a function;
/// * the close affordance lives in the chat surface's top-right
///   (`views/chat/view.rs`) and follows the chat / workspace boundary.
#[component]
fn WorkspaceHeader() -> impl IntoView {
    let i18n = use_i18n();
    let workspace = expect_context::<WorkspaceState>();
    view! {
        // `aleph-chrome-row-h` sizes the row so its vertical center lands
        // on the shared chrome y-baseline (traffic lights on macOS, brand
        // logo on web/Win/Linux). `data-tauri-drag-region=""` makes the
        // empty header area drag the window on macOS — matching the
        // chat-surface drag strip on the other side of the boundary.
        <div
            class="aleph-chrome-row-h flex items-center gap-2 px-4
                   text-xs uppercase tracking-wider text-text-tertiary"
            data-tauri-drag-region=""
        >
            <span>{t!(i18n, common.workspace_title)}</span>
            <span class="text-text-tertiary/60">
                {move || {
                    match workspace.content.get() {
                        WorkspaceContent::Empty => t_string!(i18n, common.workspace_state_idle).to_string(),
                        WorkspaceContent::ToolDetail { .. } => t_string!(i18n, common.workspace_state_tool).to_string(),
                        WorkspaceContent::Notes(_) => t_string!(i18n, common.workspace_state_notes).to_string(),
                    }
                }}
            </span>
        </div>
    }
}

#[component]
fn WorkspaceBody() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    move || match workspace.content.get() {
        WorkspaceContent::Empty => view! { <WorkspaceEmptyHero /> }.into_any(),
        WorkspaceContent::ToolDetail { run_id, tool_id } => view! {
            <ToolDetailView run_id=run_id tool_id=tool_id />
        }
        .into_any(),
        WorkspaceContent::Notes(text) => view! {
            <div class="prose-aleph max-w-none">
                <MarkdownRenderer content=text />
            </div>
        }
        .into_any(),
    }
}

/// Idle placeholder — invites the user to click a tool chip.
#[component]
fn WorkspaceEmptyHero() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col items-center justify-center
                    text-center text-text-tertiary gap-3 py-12 px-6">
            <svg xmlns="http://www.w3.org/2000/svg" class="w-10 h-10 opacity-50"
                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="9" y1="3" x2="9" y2="21"/>
                <path d="M14 8h4"/>
                <path d="M14 12h4"/>
                <path d="M14 16h4"/>
            </svg>
            <p class="text-sm font-medium text-text-secondary">{t!(i18n, common.workspace_pane)}</p>
            <p class="text-xs max-w-[24ch] leading-relaxed">
                {t!(i18n, common.workspace_hint)}
            </p>
        </div>
    }
}

/// Look the tool entry up in `ChatState.messages` and dispatch through
/// the renderer registry. Falls back to a "not found" notice if the
/// referenced run / tool_id has been evicted (e.g. user cleared chat).
#[component]
fn ToolDetailView(run_id: String, tool_id: String) -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    let workspace = expect_context::<WorkspaceState>();
    let registry = expect_context::<ToolRendererRegistry>();
    let run_id_for_entry = run_id.clone();
    let tool_id_for_entry = tool_id.clone();
    let run_id_for_payload = run_id.clone();
    let tool_id_for_payload = tool_id.clone();

    let entry = Memo::new(move |_| {
        find_tool_entry(&chat, &run_id_for_entry, &tool_id_for_entry)
    });
    let payload = Memo::new(move |_| {
        workspace.get_tool_payload(&run_id_for_payload, &tool_id_for_payload)
    });

    let run_id_for_missing = run_id.clone();
    let tool_id_for_missing = tool_id.clone();
    move || match entry.get() {
        Some(e) => view! {
            <div class="flex flex-col gap-3">
                {registry.render(&e)}
                <PayloadBlock payload=payload.get() />
            </div>
        }
        .into_any(),
        None => view! {
            <div class="flex flex-col gap-2 text-sm text-text-tertiary">
                <p>{t!(i18n, common.workspace_tool_evicted)}</p>
                <p class="text-xs">
                    "run: " <code class="font-mono">{run_id_for_missing.clone()}</code>
                </p>
                <p class="text-xs">
                    "tool: " <code class="font-mono">{tool_id_for_missing.clone()}</code>
                </p>
            </div>
        }
        .into_any(),
    }
}

/// Args + result hierarchical viewer for a tool call. Hidden entirely
/// when the payload hasn't been captured yet (no flicker between
/// renderer chip and pending payload).
///
/// Renders through [`JsonViewer`] for collapsible / type-coloured /
/// per-node copy UX. Was previously a flat `<pre>{pretty_json(...)}</pre>`
/// dump; see Round-2 panel refactor.
#[component]
fn PayloadBlock(payload: Option<ToolPayload>) -> impl IntoView {
    let Some(p) = payload else {
        return view! { <span /> }.into_any();
    };
    view! {
        <div class="flex flex-col gap-2 text-xs">
            <details class="rounded-md border border-border/60 bg-surface-sunken/60" open=true>
                <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">
                    "input"
                </summary>
                <div class="px-3 py-2 overflow-x-auto">
                    {match p.args {
                        Some(v) => view! { <JsonViewer value=v /> }.into_any(),
                        None => view! { <span class="text-text-tertiary italic">"—"</span> }.into_any(),
                    }}
                </div>
            </details>
            <details class="rounded-md border border-border/60 bg-surface-sunken/60" open=true>
                <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">
                    "result"
                </summary>
                <div class="px-3 py-2 overflow-x-auto">
                    {match p.result {
                        Some(v) => view! { <JsonViewer value=v /> }.into_any(),
                        None => view! { <span class="text-text-tertiary italic">"—"</span> }.into_any(),
                    }}
                </div>
            </details>
        </div>
    }
    .into_any()
}

/// Reactive lookup against `ChatState.messages`. The assistant message
/// for a run is named `assistant-{run_id}`, but for resilience we
/// fall-through and scan all messages if the canonical id is absent.
fn find_tool_entry(chat: &ChatState, run_id: &str, tool_id: &str) -> Option<ToolCallEntry> {
    let messages = chat.messages.get();
    let canonical_id = format!("assistant-{run_id}");
    if let Some(msg) = messages.iter().find(|m| m.id == canonical_id) {
        if let Some(found) = msg.tool_calls.iter().find(|t| t.tool_id == tool_id) {
            return Some(found.clone());
        }
    }
    messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|t| t.tool_id == tool_id)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::ChatMessage;

    fn msg_with_tools(id: &str, tools: Vec<ToolCallEntry>) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role: "assistant".into(),
            content: String::new(),
            tool_calls: tools,
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
        }
    }

}
