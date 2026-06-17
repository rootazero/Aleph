//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! Renders an **activity timeline**: every tool call in the current
//! session, derived reactively from `ChatState.messages` +
//! `WorkspaceState.tool_payloads`. Rows expand inline to show args/result
//! (file-touching tools therefore reveal their content/diff in place).
//! When no tools have run yet, shows a hero placeholder.

use crate::api::fs::{DirEntry, FsApi, ReadFileResult};
use crate::components::markdown::MarkdownRenderer;
use crate::components::tool_card::{summarize_tools, ToolCard, ToolKind};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::{FilePreview, LayoutMode, WorkspaceState};
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

/// Workspace pane root. Renders nothing when [`LayoutMode::ChatOnly`].
///
/// In team mode (`chat.team_id` is Some) shows 交付物/任务 tabs instead of the
/// single-agent ActivityTimeline + FilesDrawer. Single-agent path is unchanged.
#[component]
#[must_use]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let active_tab = RwSignal::new(0u8); // 0 = 交付物, 1 = 任务

    view! {
        // Always mounted so collapse/expand can EASE via a CSS transition
        // (mirrors the left `.aleph-sidebar`), instead of popping in/out the
        // way a `<Show>` mount/unmount would. The `workspace-collapsed`
        // modifier shrinks flex-basis + min-width to 0 over 200ms when not
        // in Split mode.
        <aside
            class="aleph-workspace-pane flex flex-col h-full
                   border-l border-border bg-surface-base/40
                   min-w-[280px] basis-[var(--aleph-workspace-w)] shrink overflow-hidden"
            class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split
        >
            <Show
                    when=move || chat.team_id.get().is_some()
                    fallback=move || {
                        // Right-pane auto-follow: stick the activity timeline to
                        // its bottom as new steps stream in, mirroring the chat
                        // list's stick-to-bottom (see `messages::MessageList`).
                        // Only follows when the user is already near the bottom
                        // (≤64px) so scrolling up to read earlier steps isn't
                        // yanked back during a live run.
                        let scroll_ref = NodeRef::<leptos::html::Div>::new();
                        let stuck = RwSignal::new(true);
                        let on_scroll = move |_ev: web_sys::Event| {
                            if let Some(el) = scroll_ref.get() {
                                let el: &web_sys::HtmlElement = &el;
                                let distance = f64::from(el.scroll_height())
                                    - f64::from(el.scroll_top())
                                    - f64::from(el.client_height());
                                stuck.set(distance <= 64.0);
                            }
                        };
                        Effect::new(move |_| {
                            // Re-run as steps / tool calls stream into the timeline.
                            let _ = chat.messages.get();
                            if let Some(el) = scroll_ref.get() {
                                if stuck.get_untracked() {
                                    let el: &web_sys::HtmlElement = &el;
                                    el.set_scroll_top(el.scroll_height());
                                }
                            }
                        });
                        view! {
                            <div
                                node_ref=scroll_ref
                                on:scroll=on_scroll
                                class="flex-1 overflow-y-auto px-4 pb-3 aleph-content-top"
                            >
                                <ActivityTimeline />
                            </div>
                            <FilesDrawer />
                        }
                    }
                >
                    // Team-mode tab header
                    <div class="flex gap-1 px-3 py-2 border-b border-border text-xs shrink-0">
                        <button
                            class=move || {
                                if active_tab.get() == 0 {
                                    "px-2 py-1 rounded bg-primary text-white"
                                } else {
                                    "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                                }
                            }
                            on:click=move |_| active_tab.set(0)
                        >"交付物"</button>
                        <button
                            class=move || {
                                if active_tab.get() == 1 {
                                    "px-2 py-1 rounded bg-primary text-white"
                                } else {
                                    "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                                }
                            }
                            on:click=move |_| active_tab.set(1)
                        >"任务"</button>
                    </div>
                    // Team-mode tab body
                    <div class="flex-1 overflow-y-auto px-3 py-2">
                        {move || {
                            if active_tab.get() == 0 {
                                view! { <TeamDeliverablesView /> }.into_any()
                            } else {
                                view! { <TeamTasksView /> }.into_any()
                            }
                        }}
                    </div>
                </Show>
            </aside>
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
    let i18n = use_i18n();

    let run_id = group.run_id.clone();
    let iteration = group.iteration;
    let run_for_focus = run_id.clone();
    let run_for_highlight = run_id.clone();
    let run_for_active = run_id.clone();

    let focused = Memo::new(move |_| workspace.is_step_focused(&run_for_highlight, iteration));
    let active = Memo::new(move |_| {
        workspace.current_iteration.with(|c| {
            c.as_ref()
                .is_some_and(|(r, i)| r == &run_for_active && *i == iteration)
        })
    });

    let narration = group.narration.clone();
    let fallback_title = summarize_tools(&group.tools)
        .into_iter()
        .map(|(k, n)| {
            let label = match k {
                ToolKind::FileEdit => t_string!(i18n, tool_card.cat_edit).to_string(),
                ToolKind::FileWrite => t_string!(i18n, tool_card.cat_write).to_string(),
                ToolKind::ApplyPatch => t_string!(i18n, tool_card.cat_patch).to_string(),
                ToolKind::FileRead => t_string!(i18n, tool_card.cat_read).to_string(),
                ToolKind::Bash => t_string!(i18n, tool_card.cat_run).to_string(),
                ToolKind::Search => t_string!(i18n, tool_card.cat_search).to_string(),
                ToolKind::Default => t_string!(i18n, tool_card.cat_tool).to_string(),
            };
            format!("{label}×{n}")
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let tools = group.tools;

    view! {
        <div
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
            {if !narration.is_empty() {
                view! {
                    <div class="text-sm text-text-primary leading-relaxed aleph-step-narration">
                        <MarkdownRenderer content=narration />
                    </div>
                }.into_any()
            } else if !fallback_title.is_empty() {
                view! {
                    <div class="text-xs text-text-tertiary font-mono">{fallback_title}</div>
                }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
            <div class="flex flex-col gap-2">
                {tools
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ToolCard run_id=run_id.clone() tool_id=tool_id tool_name=tool_name />
                        }
                    })
                    .collect_view()}
            </div>
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
                                                text-text-secondary">{f.content}</pre>
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

/// Deliverables tab — artifacts produced by the team, via teams.chat.thread.
///
/// Re-fetches whenever `chat.team_members` changes (a member finishing likely
/// produced a new artifact). The Effect only writes `items`, which is not in its
/// tracked-dep set, so it cannot self-retrigger.
#[component]
fn TeamDeliverablesView() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let items = RwSignal::new(Vec::new());
    // TODO(perf): refetches on every team_members change (each .activity event).
    // MVP-acceptable (localhost, idempotent set); future: gate on Done/Error
    // transitions or debounce.
    Effect::new(move |_| {
        let Some(team_id) = chat.team_id.get() else { return; };
        // Re-fetch when roster status changes (a member finishing likely produced output).
        let _ = chat.team_members.get();
        spawn_local(async move {
            if let Ok(thread) = crate::api::team_chat::TeamChatApi::thread(&dash, &team_id).await {
                items.set(thread.into_iter().filter(|i| i.kind == "artifact").collect::<Vec<_>>());
            }
        });
    });
    view! {
        {move || {
            let data = items.get();
            if data.is_empty() {
                view! { <div class="text-xs text-text-tertiary py-2">"暂无交付物"</div> }.into_any()
            } else {
                // Color each artifact by its producing agent's roster slot, so
                // the deliverable accent matches the chat attribution color.
                let members = chat.team_members.get_untracked();
                data.into_iter().map(|a| {
                    let color = crate::views::chat::team_events::agent_color(
                        members.iter().position(|m| m.agent_id == a.agent_id).unwrap_or(0)
                    );
                    view! {
                        <div class="border-l-2 pl-2 py-1 mb-1" style=format!("border-color:{color}")>
                            <div class="text-xs font-semibold">{a.title}</div>
                            <div class="text-[11px] opacity-70 line-clamp-3 whitespace-pre-wrap">{a.content}</div>
                        </div>
                    }
                }).collect::<Vec<_>>().into_any()
            }
        }}
    }
}

/// Tasks tab — the team's coordination tasks (CoordTask), via teams.get.
///
/// Re-fetches whenever `chat.team_members` changes. The Effect only writes
/// `tasks`, which is not in its tracked-dep set, so it cannot self-retrigger.
/// Also subscribes to `team.*.task.*` topic events so the tab live-refreshes
/// when the leader creates or updates tasks (mirrors the global KanbanView).
#[component]
fn TeamTasksView() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let tasks = RwSignal::new(Vec::new());

    // Extracted fetch closure — reused by the team_members Effect and the
    // topic-event handler so the fetch logic stays DRY.
    let refetch_tasks = move || {
        let Some(team_id) = chat.team_id.get_untracked() else { return; };
        spawn_local(async move {
            if let Ok(detail) = crate::api::teams::TeamsApi::get(&dash, &team_id).await {
                tasks.set(detail.tasks);
            }
        });
    };

    // TODO(perf): refetches on every team_members change (each .activity event).
    // MVP-acceptable (localhost, idempotent set); future: gate on Done/Error
    // transitions or debounce.
    Effect::new(move |_| {
        let Some(_team_id) = chat.team_id.get() else { return; };
        let _ = chat.team_members.get();
        refetch_tasks();
    });

    // Ask the gateway to push us `team.*.task.*` events (mirrors kanban.rs:46-55).
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });

    // React to task topic events for the current chat team (mirrors kanban.rs:57-70).
    let sub_id = dash.subscribe_events(move |evt| {
        let topic = evt.topic.as_str();
        if topic.starts_with("team.") && topic.contains(".task.") {
            refetch_tasks();
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));
    view! {
        {move || {
            let data = tasks.get();
            if data.is_empty() {
                view! { <div class="text-xs text-text-tertiary py-2">"暂无任务"</div> }.into_any()
            } else {
                data.into_iter().map(|t| view! {
                    <div class="text-xs py-1 flex justify-between gap-2 border-b border-border/40">
                        <span class="truncate">{t.subject}</span>
                        <span class="opacity-60 shrink-0">{t.status}</span>
                    </div>
                }).collect::<Vec<_>>().into_any()
            }
        }}
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
            is_final: false,
            text_finalized: false,
            agent_id: None,
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
