//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! # A pane of bodies, not a pane of modes
//!
//! What fills it is a [`WorkspaceBody`], and which bodies a session offers is
//! [`WorkspaceBody::available`]'s answer: a single-agent conversation gets
//! artifacts + canvas, a team gets deliverables + tasks + canvas. The header
//! is a tab strip over that slice, so the browser live view (spec'd as another
//! body of this same pane) costs one variant and one arm here.
//!
//! # Why the canvas body is CSS-hidden and the others are not
//!
//! [`crate::views::canvas::CanvasView`] owns three liveness wires — the
//! `is_connected`-gated loader, the `canvas.updated` topic subscription, and
//! the frame reconciler — plus the editor's camera, undo stack and in-flight
//! batch. `CANVAS.md` §6 pins them to a container that is **mounted once**:
//! unmounting it on every tab switch would refetch the library, churn the
//! subscription, and throw away the board the user is drawing on. So the
//! canvas body hangs off a `style:display` toggle that never unmounts, while
//! the other three keep their existing mount/unmount behaviour (they are
//! fetch-on-mount lists with nothing to lose).
//!
//! # The resizer writes one token, and three readers follow it
//!
//! `--aleph-workspace-w` sizes this pane (`w-[…]`), pads the chat surface
//! (`views/chat/view.rs`'s `pr-[…]`) and offsets the band chrome
//! (`app.rs`'s `right-[calc(… + 8px)]`). The band chrome is NOT inside
//! `ChatView`, so the only element whose scope reaches all three is the
//! document root — where `:root`'s 40% default lives. The publisher below
//! *removes* the property to go back to that default rather than writing a
//! second copy of the number.

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::{clamp_width, coerce_body, LayoutMode, WorkspaceBody, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

/// Localized label for a body tab.
fn body_label(body: WorkspaceBody, i18n: crate::i18n::I18nCtx) -> String {
    match body {
        WorkspaceBody::Artifacts => t_string!(i18n, common.artifacts_title).to_string(),
        WorkspaceBody::Deliverables => t_string!(i18n, common.team_deliverables).to_string(),
        WorkspaceBody::Tasks => t_string!(i18n, common.team_tasks).to_string(),
        WorkspaceBody::Canvas => t_string!(i18n, canvas.title).to_string(),
    }
}

/// Workspace pane root — always mounted, CSS-collapsed outside
/// [`LayoutMode::Split`].
#[component]
#[must_use]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let is_team = Memo::new(move |_| chat.team_id.get().is_some());
    // The body actually shown. Derived, not written back: leaving the stored
    // body alone means a user who was on `Tasks`, visited a single-agent
    // conversation and came back finds `Tasks` again — and nothing has to
    // race a team_id that arrives one frame after the session switch.
    let active = Memo::new(move |_| coerce_body(workspace.body.get(), is_team.get()));

    let pane_ref = NodeRef::<leptos::html::Aside>::new();
    let dragging = RwSignal::new(false);

    // The single writer of `--aleph-workspace-w` (see the module doc). `None`
    // removes the override so `:root`'s default takes over again.
    Effect::new(move |_| {
        let width = workspace.width_px.get();
        let Some(root) = document_root() else { return };
        match width {
            Some(px) => {
                let _ = root
                    .style()
                    .set_property("--aleph-workspace-w", &format!("{px}px"));
            }
            None => {
                let _ = root.style().remove_property("--aleph-workspace-w");
            }
        }
    });

    // Pointer-driven resize. The width is measured from the pane's own right
    // edge rather than the window's so it stays correct if the shell ever
    // grows chrome to the right of `<main>`.
    let on_pointer_down = move |ev: web_sys::PointerEvent| {
        let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let _ = target.set_pointer_capture(ev.pointer_id());
        dragging.set(true);
        ev.prevent_default();
    };
    let on_pointer_move = move |ev: web_sys::PointerEvent| {
        if !dragging.get_untracked() {
            return;
        }
        let Some(px) = width_from_pointer(pane_ref, ev.client_x()) else {
            return;
        };
        // Live preview writes the signal only; `set_width` (and its
        // `localStorage` round trip) runs once, when the drag ends.
        workspace.width_px.set(Some(px));
    };
    let on_pointer_up = move |ev: web_sys::PointerEvent| {
        if !dragging.get_untracked() {
            return;
        }
        dragging.set(false);
        if let Some(target) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.release_pointer_capture(ev.pointer_id());
        }
        if let Some(px) = width_from_pointer(pane_ref, ev.client_x()) {
            workspace.set_width(px);
        } else if let Some(px) = workspace.width_px.get_untracked() {
            workspace.set_width(px);
        }
    };

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
            node_ref=pane_ref
            class="aleph-workspace-pane absolute inset-y-0 right-0 z-20 flex flex-col
                   glass border-l border-border bg-surface-overlay/95 shadow-xl
                   min-w-[280px] w-[var(--aleph-workspace-w)] overflow-hidden"
            class:workspace-collapsed=move || workspace.mode.get() != LayoutMode::Split
        >
            // Left-edge resize handle. Absolutely positioned so it costs the
            // flex column no space; pointer capture is what keeps the drag
            // alive once the cursor leaves the 6px strip.
            <div
                class="aleph-workspace-resizer"
                class:is-dragging=move || dragging.get()
                title=move || t_string!(i18n, layout_toggle.resize_pane).to_string()
                on:pointerdown=on_pointer_down
                on:pointermove=on_pointer_move
                on:pointerup=on_pointer_up
                on:pointercancel=on_pointer_up
                on:dblclick=move |_| workspace.clear_width()
            />

            // Body tab strip. `aleph-pane-top` (not `aleph-content-top`):
            // these are real buttons, and on web the smaller content inset
            // would park them under the NotificationCenter bell — the same
            // reason `ArtifactsSurface` used to carry it.
            <div class="aleph-pane-top flex gap-1 px-3 py-2 border-b border-border text-xs shrink-0">
                // A plain map, not a keyed `<For>`: the slice is two or three
                // static variants and only changes when team-ness does, so
                // there is nothing for keying to save. Each button's `class`
                // closure reads `active` at the leaf, so selecting a tab
                // repaints two class attributes and rebuilds nothing.
                {move || {
                    WorkspaceBody::available(is_team.get())
                        .iter()
                        .copied()
                        .map(|body| view! {
                            <button
                                class=move || {
                                    if active.get() == body {
                                        "px-2 py-1 rounded bg-primary text-white"
                                    } else {
                                        "px-2 py-1 rounded text-text-secondary hover:text-text-primary"
                                    }
                                }
                                on:click=move |_| workspace.body.set(body)
                            >{body_label(body, i18n)}</button>
                        })
                        .collect_view()
                }}
            </div>

            // The three mount-on-demand bodies.
            <div
                class="flex-1 min-h-0 flex flex-col"
                style:display=move || if active.get() == WorkspaceBody::Canvas { "none" } else { "flex" }
            >
                {move || match active.get() {
                    WorkspaceBody::Artifacts => view! {
                        <crate::components::artifacts::ArtifactsSurface />
                    }.into_any(),
                    WorkspaceBody::Deliverables => view! {
                        <div class="flex-1 overflow-y-auto px-3 py-2">
                            <TeamDeliverablesView />
                        </div>
                    }.into_any(),
                    WorkspaceBody::Tasks => view! {
                        <div class="flex-1 overflow-y-auto px-3 py-2">
                            <TeamTasksView />
                        </div>
                    }.into_any(),
                    // Rendered by the keep-alive container below, never here.
                    WorkspaceBody::Canvas => ().into_any(),
                }}
            </div>

            // Canvas body — mounted once for the life of the app and hidden
            // with CSS, never unmounted (CANVAS.md §6: the three liveness
            // wires, the camera and the undo stack all live in there).
            <div
                class="flex-1 min-h-0 flex flex-col"
                style:display=move || if active.get() == WorkspaceBody::Canvas { "flex" } else { "none" }
            >
                <crate::views::canvas::CanvasView />
            </div>
        </aside>
    }
}

/// The `<html>` element, as the `HtmlElement` whose inline style carries the
/// `:root` custom-property overrides (same lookup as `team_participants.rs`).
fn document_root() -> Option<web_sys::HtmlElement> {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
}

/// Pane width implied by a pointer at `client_x`, clamped to the live
/// viewport. `None` when the pane is not laid out yet.
fn width_from_pointer(pane: NodeRef<leptos::html::Aside>, client_x: i32) -> Option<u32> {
    let el = pane.get_untracked()?;
    let right = el.get_bounding_client_rect().right();
    let raw = right - f64::from(client_x);
    let viewport = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(right);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(clamp_width(
        raw.max(0.0) as u32,
        viewport.max(0.0) as u32,
    ))
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
