//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! Renders an **activity timeline**: every tool call in the current
//! session, derived reactively from `ChatState.messages` +
//! `WorkspaceState.tool_payloads`. Rows expand inline to show args/result
//! (file-touching tools therefore reveal their content/diff in place).
//! When no tools have run yet, shows a hero placeholder.

use crate::components::json_viewer::JsonViewer;
use crate::i18n::*;
use crate::state::layout::{LayoutMode, ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Flatten all tool calls across assistant messages into ordered
/// `(run_id, tool_id, tool_name)` rows. The message id is
/// `"assistant-{run_id}"`; strip the prefix to recover the run id used as
/// the `tool_payloads` key.
fn timeline_rows(chat: &ChatState) -> Vec<(String, String, String)> {
    chat.messages
        .get()
        .iter()
        .flat_map(|m| {
            let run = m.id.strip_prefix("assistant-").unwrap_or(&m.id).to_string();
            m.tool_calls
                .iter()
                .map(move |t| (run.clone(), t.tool_id.clone(), t.tool_name.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Best-effort path extraction for a file-touching tool, so the row can
/// show a `📄 path` header. Defensive: tries the known path-bearing arg
/// keys and returns `None` for non-file tools (which then render plain).
fn file_path_of(payload: &Option<ToolPayload>) -> Option<String> {
    let args = payload.as_ref()?.args.as_ref()?;
    for key in ["path", "file_path", "filename"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Workspace pane root. Renders nothing when [`LayoutMode::ChatOnly`].
#[component]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();

    view! {
        <Show when=move || workspace.mode.get() == LayoutMode::Split>
            <aside class="aleph-workspace-pane flex flex-col h-full
                           border-l border-border bg-surface-base/40
                           min-w-[280px] basis-[66%] shrink overflow-hidden">
                <div class="flex-1 overflow-y-auto px-4 py-3">
                    <ActivityTimeline />
                </div>
            </aside>
        </Show>
    }
}

/// The reactive activity timeline.
#[component]
fn ActivityTimeline() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let rows = Memo::new(move |_| timeline_rows(&chat));

    move || {
        let data = rows.get();
        if data.is_empty() {
            view! { <WorkspaceEmptyHero /> }.into_any()
        } else {
            view! {
                <div class="flex flex-col gap-2">
                    {data
                        .into_iter()
                        .map(|(run_id, tool_id, tool_name)| {
                            view! {
                                <ActivityRow
                                    run_id=run_id
                                    tool_id=tool_id
                                    tool_name=tool_name
                                />
                            }
                        })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// One tool-call row. Click the header to expand args/result inline.
#[component]
fn ActivityRow(run_id: String, tool_id: String, tool_name: String) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();

    let tid_for_toggle = tool_id.clone();
    let tid_for_expanded = tool_id.clone();
    let tid_for_status = tool_id.clone();
    let run_for_payload = run_id.clone();
    let tid_for_payload = tool_id.clone();

    // Status + duration are looked up live from ChatState so a "running"
    // row flips to "completed" without re-deriving the whole timeline.
    let status = Memo::new(move |_| {
        chat.messages.get().iter().flat_map(|m| m.tool_calls.iter()).find_map(|t| {
            if t.tool_id == tid_for_status {
                Some((t.status.clone(), t.duration_ms))
            } else {
                None
            }
        })
    });

    let payload = Memo::new(move |_| workspace.get_tool_payload(&run_for_payload, &tid_for_payload));
    let expanded = Memo::new(move |_| workspace.is_event_expanded(&tid_for_expanded));

    let path_label = move || file_path_of(&payload.get());

    view! {
        <div class="rounded-md border border-border/60 bg-surface-sunken/40">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-3 py-2 text-left
                       hover:bg-surface-raised/40 transition-colors"
                on:click=move |_| workspace.toggle_event(&tid_for_toggle)
            >
                <span class="text-xs font-mono text-text-secondary">{tool_name.clone()}</span>
                {move || {
                    match status.get() {
                        Some((s, dur)) => {
                            let dur_txt = dur.map(|d| format!(" · {d}ms")).unwrap_or_default();
                            view! {
                                <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                                    {format!("{s}{dur_txt}")}
                                </span>
                            }
                            .into_any()
                        }
                        None => view! { <span /> }.into_any(),
                    }
                }}
                {move || match path_label() {
                    Some(p) => view! {
                        <span class="ml-auto text-[11px] font-mono text-text-tertiary truncate max-w-[50%]">
                            {format!("📄 {p}")}
                        </span>
                    }
                    .into_any(),
                    None => view! { <span class="ml-auto" /> }.into_any(),
                }}
            </button>
            <Show when=move || expanded.get()>
                <div class="px-3 pb-2">
                    <PayloadBlock payload=payload.get() />
                </div>
            </Show>
        </div>
    }
}

/// Idle placeholder — shown until the first tool call of the session.
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

/// Args + result hierarchical viewer for a tool call. Hidden when the
/// payload hasn't been captured yet.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::{ChatMessage, ToolCallEntry};

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
            timestamp: None,
        }
    }

    fn tool(id: &str, name: &str) -> ToolCallEntry {
        ToolCallEntry {
            tool_id: id.into(),
            tool_name: name.into(),
            status: "completed".into(),
            duration_ms: None,
        }
    }

    #[test]
    fn timeline_rows_flatten_in_document_order_with_run_ids() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages.set(vec![
            msg_with_tools("assistant-runA", vec![tool("t1", "read_file"), tool("t2", "search")]),
            msg_with_tools("assistant-runB", vec![tool("t3", "write_file")]),
        ]);
        let rows = timeline_rows(&chat);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("runA".to_string(), "t1".to_string(), "read_file".to_string()));
        assert_eq!(rows[1], ("runA".to_string(), "t2".to_string(), "search".to_string()));
        assert_eq!(rows[2], ("runB".to_string(), "t3".to_string(), "write_file".to_string()));
    }
}
