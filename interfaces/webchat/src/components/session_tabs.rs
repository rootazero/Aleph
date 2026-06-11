//! Top-of-chat tab strip — one tab per opened `agent_id`.
//!
//! Sits above `MessageList` inside `ChatView` so it scrolls with neither
//! the sidebar nor the workspace pane. Tabs are populated by
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
use crate::state::sessions::SessionMap;
use crate::views::chat::state::ChatState;

#[component]
#[must_use]
pub fn SessionTabs() -> impl IntoView {
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();

    install_tab_hotkeys(sessions, chat);

    view! {
        <Show when=move || sessions.tab_order.with(|o| o.len() >= 2)>
            <div class="aleph-session-tabs flex items-center gap-1 px-2 py-1
                        bg-surface-base/30 text-xs overflow-x-auto flex-shrink-0">
                <For
                    each=move || sessions.tab_order.get()
                    key=|aid| aid.clone()
                    children=move |aid: String| view! { <Tab agent_id=aid /> }
                />
            </div>
        </Show>
    }
}

/// Single tab pill — label + close button.
#[component]
fn Tab(agent_id: String) -> impl IntoView {
    let i18n = use_i18n();
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();

    let aid_for_check = agent_id.clone();
    let is_active = move || {
        sessions
            .active
            .with(|a| a.as_ref().map(|s| s == &aid_for_check).unwrap_or(false))
    };

    let aid_for_label = agent_id.clone();
    let aid_for_click = agent_id.clone();
    let aid_for_close = agent_id.clone();
    let aid_for_title = agent_id;

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
            title=aid_for_title
            on:click={
                let aid = aid_for_click;
                move |_| sessions.activate(chat, &aid)
            }
        >
            <span class="truncate">{aid_for_label}</span>
            <button
                type="button"
                class="opacity-50 hover:opacity-100 px-1 rounded
                       hover:bg-danger/20 hover:text-danger leading-none"
                title=move || t_string!(i18n, session_tabs.close_tab).to_string()
                on:click={
                    let aid = aid_for_close;
                    move |ev: web_sys::MouseEvent| {
                        ev.stop_propagation();
                        sessions.close(chat, &aid);
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
fn install_tab_hotkeys(sessions: SessionMap, chat: ChatState) {
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
                sessions.switch_by_index(chat, (digit - 1) as usize);
                return;
            }
        }
        if key.eq_ignore_ascii_case("w") {
            ev.prevent_default();
            sessions.close_active(chat);
        }
    });
}
