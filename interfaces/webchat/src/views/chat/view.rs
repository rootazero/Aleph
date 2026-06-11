//! Top-level Chat view — mounts message list + composer and hosts the
//! chat-surface drop zone (G5). Heavy pieces live in sibling modules:
//!
//! - [`super::messages`] — list + bubble + hero + send-error banner
//! - [`super::composer`] — textarea + attachments + slash palette + send

use super::composer::InputArea;
use super::events::subscribe_run_events;
use super::messages::MessageList;
use super::state::{ChatState, PendingAttachment};
use crate::components::session_tabs::SessionTabs;
use crate::components::workspace_panel::WorkspacePanel;
use crate::context::DashboardState;
use crate::i18n::*;
use crate::state::layout::WorkspaceState;
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
    let workspace = expect_context::<WorkspaceState>();

    // Subscribe to run.* events directly (no Effect — this is a one-shot mount action)
    let sub_id = subscribe_run_events(&dashboard, chat, workspace);

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
    });

    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(sub_id);
        // Tell the Gateway to stop forwarding stream.* events
        let dash = dash_for_cleanup;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
        });
    });

    // Cross-surface scroll-focus: when a timeline step becomes focused (the
    // sole writer is the right-pane step button → `WorkspaceState::focus_step`),
    // bring its chat-bubble counterpart (`#step-{run}-{iteration}`, the id set
    // in `messages.rs`) into view. The bubble already renders a highlight ring
    // via `is_step_focused`; this completes the documented scroll-focus that
    // was previously highlight-only (an off-screen counterpart was never
    // revealed). `scroll_into_view` walks ancestor scroll containers, so it
    // works inside the messages list without forcing a full-window scroll.
    Effect::new(move |_| {
        let Some((run_id, iteration)) = workspace.focused_step.get() else {
            return;
        };
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Some(el) = document.get_element_by_id(&format!("step-{run_id}-{iteration}")) {
            el.scroll_into_view();
        }
    });

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
        // Transparent — the shell's light-field shows through. Outer flex
        // row hosts the chat surface + optional workspace pane sibling.
        <div
            class="relative flex h-full"
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            // Chat surface — collapses to 33% when workspace pane is open.
            // `relative` anchors the workspace toggle (top-right corner
            // affordance) so it follows the chat-surface boundary: in
            // ChatOnly mode the toggle sits to the LEFT of the
            // window-fixed NotificationCenter bell, in Split mode the
            // chat-surface shrinks and the toggle naturally shifts left
            // toward the workspace pane edge.
            <div class="relative flex flex-col flex-1 min-w-0 h-full">
                // No chat-local drag strip or LayoutToggle here: the
                // global `aleph-main-drag-band` at the top of `<main>`
                // (see `app.rs` → `ChatBandChrome`) hosts the workspace
                // toggle on the traffic-light row and reserves the
                // macOS overlay-titlebar space uniformly across tabs.
                // On web / Win / Linux native chrome owns window drag.
                // Session tab strip — renders only when ≥2 agents are open.
                <SessionTabs />
                // Message list (scrollable) — or the welcome hero when empty
                <MessageList />
                // Input area (pinned to bottom)
                <InputArea />
            </div>
            // Workspace pane — renders only when LayoutMode::Split.
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
    let name = file.name();
    let mime_type = file.type_();
    let size = file.size() as u64;
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
