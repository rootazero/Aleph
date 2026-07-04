//! Top-of-chat tab strip — one tab per opened conversation.
//!
//! Floats over the top of `MessageList` inside `ChatView` (absolute
//! frosted overlay band) — it scrolls with neither the sidebar nor the
//! workspace pane. Tabs are populated by
//! [`SessionMap::activate`]; first activation happens implicitly when
//! the chat sidebar's dropdown picks a default agent. We only render the
//! strip when ≥2 tabs are open — a single open agent is already named in
//! the left sidebar's agent dropdown, so a one-tab strip would just
//! repeat that information and add an unsightly divider line above the
//! conversation.
//!
//! Keyboard parity with VS Code / browsers:
//!
//! - **⌘1 .. ⌘9 / Ctrl+1 .. Ctrl+9** — focus the Nth tab
//! - **⌘W / Ctrl+W** — close the active tab (drops its snapshot)
//!
//! The listener uses `prevent_default` on Cmd+W so the desktop shell
//! window doesn't close out from under the user.

use leptos::ev::keydown;
use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::sessions::{ConvId, SessionMap};
use crate::views::chat::state::ChatState;

#[component]
#[must_use]
pub fn SessionTabs() -> impl IntoView {
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();

    install_tab_hotkeys(sessions, chat, workspace);

    view! {
        <Show when=move || sessions.tab_strip_visible()>
            <div class="aleph-session-tabs flex items-center gap-1 px-2 py-1
                        text-xs overflow-x-auto">
                <For
                    each=move || sessions.order.get()
                    key=|cid| *cid
                    children=move |cid: ConvId| view! { <Tab conv=cid /> }
                />
            </div>
        </Show>
    }
}

/// Single tab pill — running dot + label + close button.
#[component]
fn Tab(conv: ConvId) -> impl IntoView {
    let i18n = use_i18n();
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();

    let is_active = move || sessions.active.with(|a| *a == Some(conv));
    let is_running = move || sessions.is_running(conv);
    let label = move || sessions.label(conv);

    view! {
        <div
            class=move || format!(
                "group flex items-center gap-1.5 pl-2.5 pr-1 py-1 rounded-md \
                 cursor-pointer transition-colors whitespace-nowrap \
                 max-w-[180px] {}",
                if is_active() {
                    "bg-primary/15 text-primary font-medium"
                } else {
                    "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                }
            )
            on:click=move |_| {
                // Only a real foreground switch invalidates the global detail
                // pane — clicking the already-active tab (activate early-
                // returns) must not drop the user's selection/pin.
                let switched = sessions.active.get_untracked() != Some(conv);
                sessions.activate(chat, conv);
                // The detail pane is global, not per-conversation — drop the
                // outgoing conversation's selection/pin so it doesn't leak
                // into the tab we just switched to (final-review F1).
                if switched {
                    if let Some(ws) = workspace {
                        ws.clear_selection();
                    }
                }
            }
        >
            // 进行中红点（隐现）。
            <Show when=is_running>
                <span
                    class="w-1.5 h-1.5 rounded-full bg-danger animate-pulse shrink-0"
                    title="running"
                />
            </Show>
            <span class="truncate">{label}</span>
            <button
                type="button"
                class="opacity-50 hover:opacity-100 px-1 rounded
                       hover:bg-danger/20 hover:text-danger leading-none"
                title=move || t_string!(i18n, session_tabs.close_tab).to_string()
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    // Closing the foreground tab promotes a neighbour: the
                    // dead conversation's selection/pin would otherwise stay
                    // on the pane forever (its ChatState is dropped, so no
                    // run_complete will ever unpin). Closing a *background*
                    // tab leaves the foreground untouched — don't clear then.
                    let was_active = sessions.active.get_untracked() == Some(conv);
                    sessions.close(chat, conv);
                    if was_active {
                        if let Some(ws) = workspace {
                            ws.clear_selection();
                        }
                    }
                }
            >
                "×"
            </button>
        </div>
    }
}

/// Bind ⌘1..9 / ⌘W (plus Ctrl- variants for non-mac browsers / Windows).
/// Listener is leaked deliberately — same pattern as `state::hotkey::install`.
fn install_tab_hotkeys(sessions: SessionMap, chat: ChatState, workspace: Option<WorkspaceState>) {
    window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        let mod_pressed = ev.meta_key() || ev.ctrl_key();
        if !mod_pressed || ev.alt_key() {
            return;
        }
        let key = ev.key();
        // ⌘1..9 → switch by index. Browser default for ⌘1..8 is "switch
        // browser tab", which we override since the panel is the foreground
        // app. ⌘9 is "last tab" in browsers — mirroring it would clash
        // with our "9th tab" semantics, but our N=9 (last numeric) is
        // close enough that we just claim it too.
        if let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10)) {
            if (1..=9).contains(&digit) && key.len() == 1 {
                ev.prevent_default();
                // See Tab's on:click — same global-pane leak (final-review
                // F1), same was-active guard: ⌘N onto the current tab is a
                // no-op switch and must not drop the selection/pin.
                let idx = (digit - 1) as usize;
                let target = sessions.order.with_untracked(|o| o.get(idx).copied());
                let switched = target.is_some() && target != sessions.active.get_untracked();
                sessions.switch_by_index(chat, idx);
                if switched {
                    if let Some(ws) = workspace {
                        ws.clear_selection();
                    }
                }
                return;
            }
        }
        if key.eq_ignore_ascii_case("w") {
            ev.prevent_default();
            // ⌘W closes the *active* tab by definition — the promoted
            // neighbour must not inherit the dead conversation's
            // selection/pin (see the × button above). No-tab boot state
            // (`active == None`) has nothing to clear.
            let had_active = sessions.active.get_untracked().is_some();
            sessions.close_active(chat);
            if had_active {
                if let Some(ws) = workspace {
                    ws.clear_selection();
                }
            }
        }
    });
}
