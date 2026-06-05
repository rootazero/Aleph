//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! Renders an **activity timeline**: every tool call in the current
//! session, derived reactively from `ChatState.messages` +
//! `WorkspaceState.tool_payloads`. Rows expand inline to show args/result
//! (file-touching tools therefore reveal their content/diff in place).
//! When no tools have run yet, shows a hero placeholder.

use crate::api::fs::{DirEntry, FsApi, ReadFileResult};
use crate::components::json_viewer::JsonViewer;
use crate::context::DashboardState;
use crate::i18n::*;
use crate::state::layout::{FilePreview, LayoutMode, ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::views::chat::messages::run_id_from_message_id;

/// One agent step (Think→Act iteration) for the workspace timeline: the
/// iteration's narration plus the tool calls it triggered.
#[derive(Clone, PartialEq)]
struct StepGroup {
    run_id: String,
    iteration: usize,
    narration: String,
    tools: Vec<(String, String)>, // (tool_id, tool_name)
}

/// Build iteration-grouped steps from the chat transcript. Each assistant
/// bubble carrying an `iteration` tag becomes one step; its content is the
/// narration and its `tool_calls` are the step's tools.
fn timeline_groups(chat: &ChatState) -> Vec<StepGroup> {
    chat.messages
        .get()
        .iter()
        .filter(|m| m.role == "assistant")
        .filter_map(|m| {
            let iteration = m.iteration?;
            Some(StepGroup {
                run_id: run_id_from_message_id(&m.id),
                iteration,
                narration: m.content.clone(),
                tools: m
                    .tool_calls
                    .iter()
                    .map(|t| (t.tool_id.clone(), t.tool_name.clone()))
                    .collect(),
            })
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
                <FilesDrawer />
            </aside>
        </Show>
    }
}

/// The reactive activity timeline — one card per agent step (iteration).
#[component]
fn ActivityTimeline() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let groups = Memo::new(move |_| timeline_groups(&chat));

    move || {
        let data = groups.get();
        if data.is_empty() {
            view! { <WorkspaceEmptyHero /> }.into_any()
        } else {
            view! {
                <div class="flex flex-col gap-3">
                    {data
                        .into_iter()
                        .map(|g| view! { <StepCard group=g /> })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// One agent step: iteration header + narration + its tool rows. Clicking the
/// header focuses the step (cross-highlight with the chat bubble).
#[component]
fn StepCard(group: StepGroup) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();

    let run_id = group.run_id.clone();
    let iteration = group.iteration;
    let run_for_focus = run_id.clone();
    let run_for_highlight = run_id.clone();
    let run_for_active = run_id.clone();

    let focused = Memo::new(move |_| workspace.is_step_focused(&run_for_highlight, iteration));
    let active = Memo::new(move |_| {
        workspace
            .current_iteration
            .with(|c| c.as_ref().is_some_and(|(r, i)| r == &run_for_active && *i == iteration))
    });

    let narration = group.narration.clone();
    let has_narration = !narration.is_empty();
    let tools = group.tools.clone();

    view! {
        <div
            id=format!("group-{}-{}", run_id, iteration)
            class=move || {
                let base = "rounded-lg border p-2 flex flex-col gap-2 transition-colors";
                if focused.get() {
                    format!("{base} border-primary/60 bg-primary/5")
                } else {
                    format!("{base} border-border/50 bg-surface-sunken/40")
                }
            }
        >
            <button
                type="button"
                class="flex items-center gap-2 text-left"
                on:click=move |_| workspace.focus_step(run_for_focus.clone(), iteration)
            >
                <span class="text-[10px] font-mono uppercase tracking-wider text-text-tertiary">
                    {format!("#{iteration}")}
                </span>
                <Show when=move || active.get()>
                    <span class="inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                </Show>
            </button>
            <Show when=move || has_narration>
                <p class="text-xs text-text-secondary whitespace-pre-wrap leading-relaxed">
                    {narration.clone()}
                </p>
            </Show>
            <div class="flex flex-col gap-2">
                {tools
                    .clone()
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ActivityRow run_id=run_id.clone() tool_id=tool_id tool_name=tool_name />
                        }
                    })
                    .collect_view()}
            </div>
        </div>
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
        chat.messages
            .get()
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .find_map(|t| {
                if t.tool_id == tid_for_status {
                    Some((t.status.clone(), t.duration_ms))
                } else {
                    None
                }
            })
    });

    let payload =
        Memo::new(move |_| workspace.get_tool_payload(&run_for_payload, &tid_for_payload));
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

/// Bottom drawer: collapsible project file tree + read-only preview.
#[component]
fn FilesDrawer() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let entries = RwSignal::new(Vec::<DirEntry>::new());
    let cur_path = RwSignal::new(Option::<String>::None);

    // Effect A — seed the path when the drawer opens. Prefer the active
    // project root, else the first allowed root. Skips reseeding once a
    // path is set so folder navigation isn't clobbered.
    Effect::new(move |_| {
        if !workspace.files_drawer_open.get() {
            return;
        }
        if cur_path.get().is_some() {
            return;
        }
        match chat.active_project_root.get() {
            Some(root) => cur_path.set(Some(root)),
            None => {
                let dash = dashboard;
                spawn_local(async move {
                    if let Ok(roots) = FsApi::allowed_roots(&dash).await {
                        if let Some(r) = roots.first() {
                            cur_path.set(Some(r.path.clone()));
                        }
                    }
                });
            }
        }
    });

    // Effect B — list entries whenever the path changes. Must NOT write
    // `cur_path` (only `entries`), otherwise it would re-trigger itself
    // and fire a redundant `list_dir`.
    Effect::new(move |_| {
        if !workspace.files_drawer_open.get() {
            return;
        }
        let Some(path) = cur_path.get() else {
            return;
        };
        let dash = dashboard;
        spawn_local(async move {
            if let Ok(listing) = FsApi::list_dir(&dash, &path, false).await {
                entries.set(listing.entries);
            }
        });
    });

    // Reset drawer navigation when the active project changes so a session
    // switch doesn't leave the previous project's listing behind. Reads
    // active_project_root only; writes cur_path/entries (never reads them)
    // → cannot self-retrigger. Effect A then reseeds from the new root.
    Effect::new(move |_| {
        let _ = chat.active_project_root.get();
        cur_path.set(None);
        entries.set(Vec::new());
    });

    view! {
        <div class="border-t border-border bg-surface-base/60">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-4 py-2 text-left text-xs
                       uppercase tracking-wider text-text-tertiary hover:text-text-secondary"
                on:click=move |_| workspace.toggle_files_drawer()
            >
                <span>{move || t_string!(i18n, common.workspace_files).to_string()}</span>
                <span class="ml-auto">
                    {move || if workspace.files_drawer_open.get() { "▾" } else { "▸" }}
                </span>
            </button>
            <Show when=move || workspace.files_drawer_open.get()>
                <div class="flex max-h-[40vh] border-t border-border/60">
                    <div class="w-1/3 overflow-y-auto border-r border-border/60 p-2 text-xs">
                        <For
                            each=move || entries.get()
                            key=|e| e.path.clone()
                            children=move |e: DirEntry| {
                                let path = e.path.clone();
                                let is_dir = e.is_dir;
                                view! {
                                    <button
                                        type="button"
                                        class="w-full text-left truncate px-1 py-0.5 rounded
                                               hover:bg-surface-raised/50"
                                        on:click=move |_| {
                                            if is_dir {
                                                cur_path.set(Some(path.clone()));
                                            } else {
                                                let dash = dashboard;
                                                let p = path.clone();
                                                spawn_local(async move {
                                                    if let Ok(ReadFileResult { path, content, truncated }) =
                                                        FsApi::read_file(&dash, &p).await
                                                    {
                                                        workspace.select_file(Some(FilePreview {
                                                            path,
                                                            content,
                                                            truncated,
                                                        }));
                                                    }
                                                });
                                            }
                                        }
                                    >
                                        {if e.is_dir {
                                            format!("📁 {}", e.name)
                                        } else {
                                            format!("📄 {}", e.name)
                                        }}
                                    </button>
                                }
                            }
                        />
                    </div>
                    <div class="flex-1 overflow-auto p-2">
                        {move || match workspace.selected_file.get() {
                            Some(f) => view! {
                                <div class="flex flex-col gap-1">
                                    <div class="text-[11px] font-mono text-text-tertiary truncate">
                                        {f.path.clone()}
                                        {if f.truncated { " (truncated)" } else { "" }}
                                    </div>
                                    <pre class="text-xs whitespace-pre-wrap break-words font-mono
                                                text-text-secondary">{f.content.clone()}</pre>
                                </div>
                            }
                            .into_any(),
                            None => view! {
                                <p class="text-xs text-text-tertiary italic">
                                    {t!(i18n, common.workspace_files_hint)}
                                </p>
                            }
                            .into_any(),
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::{ChatMessage, ToolCallEntry};

    fn step_msg(
        id: &str,
        iteration: Option<usize>,
        content: &str,
        tools: Vec<ToolCallEntry>,
    ) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "assistant".into(),
            content: content.to_string(),
            tool_calls: tools,
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            iteration,
            timestamp: None,
        }
    }

    #[test]
    fn timeline_groups_one_per_tagged_bubble() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages.set(vec![
            step_msg(
                "intermediate-r1-1",
                Some(1),
                "search images",
                vec![ToolCallEntry {
                    tool_id: "t1".into(),
                    tool_name: "search".into(),
                    status: "completed".into(),
                    duration_ms: Some(5),
                }],
            ),
            step_msg(
                "assistant-r1",
                Some(2),
                "write html",
                vec![ToolCallEntry {
                    tool_id: "t2".into(),
                    tool_name: "write".into(),
                    status: "running".into(),
                    duration_ms: None,
                }],
            ),
        ]);

        let groups = timeline_groups(&chat);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].run_id, "r1");
        assert_eq!(groups[0].iteration, 1);
        assert_eq!(groups[0].narration, "search images");
        assert_eq!(
            groups[0].tools,
            vec![("t1".to_string(), "search".to_string())]
        );
        assert_eq!(groups[1].iteration, 2);
        assert_eq!(
            groups[1].tools,
            vec![("t2".to_string(), "write".to_string())]
        );
    }

    #[test]
    fn timeline_groups_skips_untagged_messages() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages
            .set(vec![step_msg("assistant-r1", None, "no tag", vec![])]);
        assert!(timeline_groups(&chat).is_empty());
    }
}
