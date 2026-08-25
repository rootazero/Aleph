//! Top-level Chat view — mounts message list + composer and hosts the
//! chat-surface drop zone (G5). Heavy pieces live in sibling modules:
//!
//! - [`super::messages`] — list + bubble + hero + send-error banner
//! - [`super::composer`] — textarea + attachments + slash palette + send
//!
//! `MainContent` (`app.rs`) keeps this component permanently mounted at `/`
//! (CSS `display` toggling, never unmounted) even while another mode is in
//! view. Do NOT mount a second `<ChatView />` anywhere else — every mount
//! independently `subscribe_topic("stream.*")` / `"team.*"` and tears the
//! subscription down again in `on_cleanup`, so a second instance unmounting
//! (e.g. navigating away from a page that embedded it) would kill this one's
//! event stream too. `components::project_page::RoomChat` (P2 Task 8) needs
//! a live chat surface inside a *different* route and works around this by
//! mounting `MessageList` + `InputArea` directly instead — both are inert
//! with respect to topic subscriptions; only `ChatView` itself owns those.

use super::composer::InputArea;
use super::messages::MessageList;
use super::state::{ChatState, PendingAttachment};
use super::team_events::subscribe_team_events;
use crate::components::team_participants::TeamParticipants;
use crate::components::team_task_strip::TaskDrawerOpen;
use crate::components::team_task_strip::TeamTaskDrawer;
use crate::components::workspace_panel::WorkspacePanel;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::state::layout::{LayoutMode, WorkspaceState};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

/// Top-level Chat view component.
#[component]
#[must_use]
pub fn ChatView() -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = expect_context::<DashboardState>();
    // ChatState is provided once at the app root so the chat sidebar
    // (left column) and this view share one session / agent selection.
    let chat = expect_context::<ChatState>();
    // WorkspaceState is optional during early boot races; when present, the
    // chat surface reserves the workspace pane's width on the right in Split
    // mode so message/composer max-width centers inside the *visible* chat
    // area instead of drifting under the floating workspace pane.
    let workspace = use_context::<WorkspaceState>();

    // Team chat: subscribe to team.<id>.* events alongside the single-agent stream.
    // Harmless when not in team mode (the handler short-circuits on topic prefix).
    let team_sub_id = subscribe_team_events(&dashboard, chat);

    // Team chat: hydrate the task strip/drawer from teams.list_tasks whenever
    // the active team changes (and we're connected). Incremental updates after
    // this come from the team.<id>.task.<verb> branch in team_events.rs.
    let dash_for_tasks = dashboard;
    Effect::new(move |_| {
        let Some(team_id) = chat.team_id.get() else {
            chat.team_tasks.set(Vec::new());
            return;
        };
        if !dash_for_tasks.is_connected.get() {
            return;
        }
        let chat2 = chat;
        spawn_local(async move {
            if let Ok(tasks) = crate::api::teams::TeamsApi::list_tasks(
                &dash_for_tasks,
                &team_id,
                crate::api::teams::TaskFilter::default(),
            )
            .await
            {
                chat2.team_tasks.set(tasks);
            }
        });
    });

    // Tell the Gateway to start forwarding stream.* events
    // (backend publishes events with method "stream.run_accepted", "stream.response_chunk", etc.)
    // Wait until connected before subscribing, since ChatView may mount before WebSocket is ready.
    let dash_for_sub = dashboard;
    spawn_local(async move {
        // Poll until connected (max ~5s)
        for _ in 0..50 {
            if dash_for_sub.is_connected.get_untracked() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }
        if let Err(e) = dash_for_sub.subscribe_topic("stream.*").await {
            web_sys::console::error_1(&format!("Failed to subscribe stream events: {e}").into());
        }
        // `stream.ask_user` is a one-shot push, so a panel that reloads (or
        // connects late) while the agent is parked on a question would never
        // see it — and the parked tool would sit blocked until it timed out.
        // Seed from the authoritative registry, exactly as the approval card
        // seeds from `exec.approvals.pending`.
        match crate::api::ClarificationApi::list_pending(&dash_for_sub).await {
            Ok(list) => dash_for_sub.pending_clarifications.set(list),
            Err(e) => web_sys::console::error_1(
                &format!("Failed to load pending clarifications: {e}").into(),
            ),
        }
    });

    // Subscribe team.* topic (backend fan-out for team chat events).
    let dash_for_team_sub = dashboard;
    spawn_local(async move {
        for _ in 0..50 {
            if dash_for_team_sub.is_connected.get_untracked() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }
        if let Err(e) = dash_for_team_sub.subscribe_topic("team.*").await {
            web_sys::console::error_1(&format!("Failed to subscribe team events: {e}").into());
        }
    });

    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(team_sub_id);
        // Tell the Gateway to stop forwarding stream.* and team.* events
        let dash = dash_for_cleanup;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
            let _ = dash.unsubscribe_topic("team.*").await;
        });
    });

    // Team task drawer open-state — provided at the chat-view level so both the
    // composer's TeamTaskStrip and the chat-column TeamTaskDrawer (Task 6) read
    // the same signal via expect_context.
    let task_drawer_open = RwSignal::new(false);
    provide_context(TaskDrawerOpen(task_drawer_open));

    // ---- G5: chat-surface drop zone ----
    // Listening on the root div so a Finder/Explorer drop anywhere over
    // the chat (messages list, composer, anywhere in between) routes the
    // files through the same attachment pipeline as the paperclip input.
    let on_dragover = move |ev: web_sys::DragEvent| {
        // Only react to file drags — keeps text-selection drag inside
        // bubbles silent. `types` reports "Files" when a file is over us.
        if let Some(dt) = ev.data_transfer() {
            let types = dt.types();
            let mut has_files = false;
            for i in 0..types.length() {
                if let Some(s) = types.get(i).as_string() {
                    if s == "Files" {
                        has_files = true;
                        break;
                    }
                }
            }
            if has_files {
                ev.prevent_default(); // mandatory for drop to fire
                chat.is_dragging_files.set(true);
            }
        }
    };
    let on_dragleave = move |_: web_sys::DragEvent| {
        chat.is_dragging_files.set(false);
    };
    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        chat.is_dragging_files.set(false);
        let Some(dt) = ev.data_transfer() else { return };
        let Some(files) = dt.files() else { return };
        let attachments = chat.pending_attachments;
        for i in 0..files.length() {
            let Some(file) = files.get(i) else { continue };
            ingest_dropped_file(file, attachments);
        }
    };

    view! {
        // Transparent — the shell's light-field shows through. `relative` is
        // the positioning ancestor for the workspace pane, which now FLOATS
        // over the chat surface as a right-anchored overlay (see
        // `WorkspacePanel`); `overflow-hidden` clips it while it's slid
        // off-screen-right when collapsed, so `<main>` grows no horizontal
        // scrollbar.
        <div
            class="relative flex h-full overflow-hidden"
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            // Chat surface — yields ~40% to the workspace pane when it's open.
            // `relative` anchors the workspace toggle (top-right corner
            // affordance) so it follows the chat-surface boundary: in
            // ChatOnly mode the toggle sits to the LEFT of the
            // window-fixed NotificationCenter bell, in Split mode the
            // chat-surface shrinks and the toggle naturally shifts left
            // toward the workspace pane edge.
            <div class=move || {
                let base = "relative flex flex-col flex-1 min-w-0 h-full";
                let split = workspace.map(|w| w.mode.get() == LayoutMode::Split).unwrap_or(false);
                if split {
                    format!("{base} pr-[var(--aleph-workspace-w)]")
                } else {
                    base.to_string()
                }
            }>
                // No chat-local drag strip or LayoutToggle here: the
                // global `aleph-main-drag-band` at the top of `<main>`
                // (see `app.rs` → `ChatBandChrome`) hosts the workspace
                // toggle on the traffic-light row and reserves the
                // macOS overlay-titlebar space uniformly across tabs.
                // On web / Win / Linux native chrome owns window drag.
                // Overlap container: the scroll area extends to the full height
                // and the composer FLOATS over the bottom (real backdrop blur —
                // messages frost as they flow behind it).
                <div class="relative flex-1 min-h-0">
                    // Message list (scrollable) — or the welcome hero when empty
                    <MessageList />
                    // Team participants — top-left avatar cluster + popover
                    // (replaces the old left roster rail). Top-left keeps it
                    // clear of the band's workspace toggle + notification bell,
                    // which live top-right. Team mode only.
                    //
                    // The macOS `aleph-main-drag-band` (app.rs, z-50,
                    // `-webkit-app-region: drag`) floats over the top 30px of
                    // `<main>`. This full-width wrapper sits ABOVE the band
                    // (`z-[60]`) but is `pointer-events-none`: window-drag and
                    // the top chrome (sidebar/workspace toggles, notification
                    // bell) stay reachable THROUGH it — it carves no `no-drag`
                    // hole and steals no clicks. Only the roster's own cluster
                    // button + popover opt back in with `pointer-events-auto` +
                    // `aleph-no-drag` (see team_participants.rs); the expanded
                    // pill bar is display-only, so the band drags over it.
                    <Show when=move || chat.team_id.get().is_some()>
                        <div class="absolute top-0 inset-x-0 z-[60] pointer-events-none">
                            <TeamParticipants />
                        </div>
                    </Show>
                    // Input area (floating glass bar pinned over the flow)
                    <InputArea />
                    <Show when=move || chat.team_id.get().is_some()>
                        <TeamTaskDrawer />
                    </Show>
                </div>
            </div>
            // Workspace pane — always mounted; eases open/closed on Split.
            <WorkspacePanel />
            // Drop overlay — only visible while a file is hovering.
            <Show when=move || chat.is_dragging_files.get()>
                <div class="absolute inset-0 z-30 pointer-events-none
                            flex items-center justify-center
                            bg-primary/10 border-2 border-dashed border-primary/40 rounded-lg
                            backdrop-blur-[1px]">
                    <div class="flex flex-col items-center gap-2 text-primary font-medium">
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-10 h-10" viewBox="0 0 24 24"
                             fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
                            <polyline points="17 8 12 3 7 8"/>
                            <line x1="12" y1="3" x2="12" y2="15"/>
                        </svg>
                        <span class="text-sm">{t!(i18n, chat.drop_to_attach)}</span>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Read a single dropped `web_sys::File` into base64 and append it to the
/// shared pending-attachments list. Mirrors the per-file logic used by the
/// paperclip input — kept as a free fn so the drop handler stays tight.
fn ingest_dropped_file(file: web_sys::File, attachments: RwSignal<Vec<PendingAttachment>>) {
    use super::composer::attachments::{MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_SIZE_BYTES};

    let name = file.name();
    let mime_type = file.type_();
    let size = file.size() as u64;
    if size > MAX_ATTACHMENT_SIZE_BYTES {
        web_sys::console::warn_1(
            &format!(
                "skipping dropped file '{name}': {size} bytes exceeds the {} MB limit",
                MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
            )
            .into(),
        );
        return;
    }
    let accepted = attachments.with_untracked(|list| list.len());
    if accepted >= MAX_ATTACHMENT_COUNT {
        web_sys::console::warn_1(
            &format!(
                "skipping dropped file '{name}': already at the {MAX_ATTACHMENT_COUNT}-file limit"
            )
            .into(),
        );
        return;
    }
    let Ok(reader) = web_sys::FileReader::new() else {
        return;
    };
    let reader_clone = reader.clone();
    let file_name = name;
    let file_mime = if mime_type.is_empty() {
        "application/octet-stream".to_string()
    } else {
        mime_type
    };
    let onload = Closure::wrap(Box::new(move || {
        if let Ok(result) = reader_clone.result() {
            if let Some(data_url) = result.as_string() {
                let base64_data = data_url.split(',').nth(1).unwrap_or("").to_string();
                let attachment = PendingAttachment {
                    name: file_name.clone(),
                    mime_type: file_mime.clone(),
                    data_base64: base64_data,
                    size,
                };
                attachments.update(|list| list.push(attachment));
            }
        }
    }) as Box<dyn Fn()>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    onload.forget();
    let _ = reader.read_as_data_url(&file);
}
