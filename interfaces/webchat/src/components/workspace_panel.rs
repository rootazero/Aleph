//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! In team mode the body is the deliverables/tasks tabs. The single-agent body
//! is [`crate::components::artifacts::ArtifactsSurface`] — what this session
//! produced (images, files, exports), read from `artifacts.list`.

use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::state::layout::{LayoutMode, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Workspace pane root. Renders nothing when [`LayoutMode::ChatOnly`].
///
/// In team mode (`chat.team_id` is Some) shows deliverable/task tabs instead of
/// the single-agent artifacts surface.
#[component]
#[must_use]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let active_tab = RwSignal::new(0u8); // 0 = deliverables, 1 = tasks
    let i18n = use_i18n();

    view! {
        // Always mounted so collapse/expand can EASE via a CSS transition.
        // FLOATS over the chat surface as an opaque overlay (same idiom as
        // the composer's project / model-picker popovers: `glass` +
        // `bg-surface-overlay` + `shadow-xl`) instead of being a flex sibling
        // column — so opening/closing it no longer reflows the chat detail.
        // `absolute inset-y-0 right-0` anchors it to the right edge of the
        // ChatView's `relative` root; the `workspace-collapsed` modifier
        // slides it off-screen right + fades over 200ms when not in Split.
        // Width still reads `--aleph-workspace-w` so the band chrome
        // (label + LayoutToggle in app.rs) stays glued to its leading edge.
        <aside
            class="aleph-workspace-pane absolute inset-y-0 right-0 z-20 flex flex-col
                   glass border-l border-border bg-surface-overlay/95 shadow-xl
                   min-w-[280px] w-[var(--aleph-workspace-w)] overflow-hidden"
            class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split
        >
            <Show
                    when=move || chat.team_id.get().is_some()
                    fallback=move || view! {
                        <crate::components::artifacts::ArtifactsSurface />
                    }
                >
                    // Team-mode tab header. `aleph-content-top` clears the
                    // macOS overlay-titlebar drag band (30px) so the tabs
                    // aren't jammed under the traffic lights / band (their top
                    // would otherwise be unclickable); on web/Win/Linux the
                    // token is the smaller sidebar-logo inset, aligning the
                    // header with the brand row. Single-agent path already
                    // applies the same token to its scroll container.
                    <div class="aleph-content-top flex gap-1 px-3 py-2 border-b border-border text-xs shrink-0">
                        <button
                            class=move || {
                                if active_tab.get() == 0 {
                                    "px-2 py-1 rounded bg-primary text-white"
                                } else {
                                    "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                                }
                            }
                            on:click=move |_| active_tab.set(0)
                        >{t!(i18n, common.team_deliverables)}</button>
                        <button
                            class=move || {
                                if active_tab.get() == 1 {
                                    "px-2 py-1 rounded bg-primary text-white"
                                } else {
                                    "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                                }
                            }
                            on:click=move |_| active_tab.set(1)
                        >{t!(i18n, common.team_tasks)}</button>
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
    let i18n = use_i18n();
    // TODO(perf): refetches on every team_members change (each .activity event).
    // MVP-acceptable (localhost, idempotent set); future: gate on Done/Error
    // transitions or debounce.
    Effect::new(move |_| {
        let Some(team_id) = chat.team_id.get() else {
            return;
        };
        // Re-fetch when roster status changes (a member finishing likely produced output).
        let _ = chat.team_members.get();
        spawn_local(async move {
            if let Ok(thread) = crate::api::team_chat::TeamChatApi::thread(&dash, &team_id).await {
                items.set(
                    thread
                        .into_iter()
                        .filter(|i| i.kind == "artifact")
                        .collect::<Vec<_>>(),
                );
            }
        });
    });
    view! {
        {move || {
            let data = items.get();
            if data.is_empty() {
                view! { <div class="text-xs text-text-tertiary py-2">{t!(i18n, common.team_no_deliverables)}</div> }.into_any()
            } else {
                // Color each artifact by its producing agent, through the SAME
                // id-hashed palette the chat bubbles and roster use. The old
                // roster-slot lookup was a second, independent color source:
                // it fell back to slot 0 for any agent missing from the roster
                // and drifted from the bubble accent whenever roster order and
                // hash order disagreed — i.e. almost always.
                data.into_iter().map(|a| {
                    let color = crate::views::chat::agent_identity::agent_color_for_id(&a.agent_id);
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
    let i18n = use_i18n();

    // Extracted fetch closure — reused by the team_members Effect and the
    // topic-event handler so the fetch logic stays DRY.
    let refetch_tasks = move || {
        let Some(team_id) = chat.team_id.get_untracked() else {
            return;
        };
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
        let Some(_team_id) = chat.team_id.get() else {
            return;
        };
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
                view! { <div class="text-xs text-text-tertiary py-2">{t!(i18n, common.team_no_tasks)}</div> }.into_any()
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
