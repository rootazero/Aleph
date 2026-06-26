//! Phone Chat conversation view. Manual iOS chrome (a dynamic title isn't
//! expressible through PhoneShell's `&'static str` title, and the body must be
//! flush so MessageList controls its own scroll) reusing PhoneTabBar. Renders
//! the shared `MessageList` + `PhoneComposer` against the app-root ChatState.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::chat::composer::PhoneComposer;
use crate::platform::phone::shell::PhoneTabBar;
use crate::views::chat::messages::MessageList;

#[component]
#[must_use]
pub fn PhoneChatThread() -> impl IntoView {
    let navigate = use_navigate();
    let back = move |_| navigate("/", NavigateOptions::default());

    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:var(--color-surface);"
        >
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; gap:8px; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                <button
                    style="position:absolute; left:10px; top:50%; transform:translateY(-10%); display:flex; align-items:center; gap:2px; background:none; border:0; cursor:pointer; color:var(--color-primary); font:inherit; font-size:16px; padding:4px 6px 4px 0;"
                    on:click=back
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 6 9 12 15 18"></polyline></svg>
                    "Chat"
                </button>
                <span style="width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);">"Conversation"</span>
            </div>

            <div style="flex:1; min-height:0; display:flex; flex-direction:column;">
                <MessageList/>
            </div>

            <PhoneComposer/>
            <PhoneTabBar/>
        </div>
    }
}
