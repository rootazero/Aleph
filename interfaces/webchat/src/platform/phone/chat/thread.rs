//! Phone Chat conversation surface — the Chat tab landing. Manual iOS chrome
//! (a dynamic title isn't expressible through PhoneShell's `&'static str` title,
//! and the body must be flush so MessageList controls its own scroll) reusing
//! PhoneTabBar. The top bar carries a history button (left) and a new-chat
//! button (right); there is no back button because this surface is the tab root.
//! Renders the shared `MessageList` + `PhoneComposer` against the app-root
//! ChatState (preserved across tab switches; empty on a cold boot = new chat).

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::i18n::{t_string, use_i18n};
use crate::platform::phone::chat::composer::PhoneComposer;
use crate::platform::phone::shell::PhoneTabBar;
use crate::state::sessions::SessionMap;
use crate::views::chat::messages::MessageList;
use crate::views::chat::ChatState;

#[component]
#[must_use]
pub fn PhoneChatThread() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();
    let navigate = use_navigate();
    let open_history = move |_| navigate("/chat/history", NavigateOptions::default());
    // New chat: open a fresh conversation and clear the session (agent_id is
    // preserved by `clear_session`) → MessageList falls back to the welcome
    // hero. Already on the chat surface, so no navigation is needed.
    //
    // `start_new` is what the wide ＋ button does, and skipping it was not
    // cosmetic: without a new `ConvId` the previous conversation's route and
    // `session_key` stayed attached to the one view this surface has, so a run
    // still finishing on the session the user just walked away from rendered
    // into the empty new chat.
    let new_chat = move |_: web_sys::MouseEvent| {
        let agent_id = chat.agent_id.get_untracked().unwrap_or_default();
        sessions.start_new(chat, &agent_id, t_string!(i18n, chat.new_chat).to_string());
        chat.clear_session();
    };

    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:var(--color-surface);"
        >
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                <button
                    style="position:absolute; left:8px; top:50%; transform:translateY(-10%); display:flex; align-items:center; justify-content:center; background:none; border:0; cursor:pointer; color:var(--color-primary); padding:6px;"
                    on:click=open_history
                    aria-label="History"
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"></circle><polyline points="12 7 12 12 15.5 14"></polyline></svg>
                </button>
                <span style="width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);">"Aleph"</span>
                <button
                    style="position:absolute; right:8px; top:50%; transform:translateY(-10%); display:flex; align-items:center; justify-content:center; background:none; border:0; cursor:pointer; color:var(--color-primary); padding:6px;"
                    on:click=new_chat
                    aria-label="New chat"
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"></path><path d="M16.5 3.5a 2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"></path></svg>
                </button>
            </div>

            <div style="flex:1; min-height:0; display:flex; flex-direction:column;">
                <MessageList/>
            </div>

            <PhoneComposer/>
            <PhoneTabBar/>
        </div>
    }
}
