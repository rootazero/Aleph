//! Native iPhone Chat screens (single-agent). Mirrors the Settings phone
//! pattern: a session-list landing (`/`) drilling into a conversation
//! (`/chat`). Reuses ChatState / ChatApi / MessageList; only the list and a
//! minimal composer are phone-specific.

pub mod composer;
pub mod list;
pub mod thread;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::context::DashboardState;
use crate::state::layout::WorkspaceState;
use crate::views::chat::events::subscribe_run_events;
use crate::views::chat::ChatState;

use self::list::PhoneChatList;
use self::thread::PhoneChatThread;

/// Phone Chat router. Owns the `run.*` streaming subscription (mirrors the wide
/// `ChatView`); exactly one of {ChatView, PhoneChat} mounts per form factor, so
/// there is no double-subscribe. Renders the list at `/` and the thread at
/// `/chat`.
#[component]
#[must_use]
pub fn PhoneChat() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let workspace = expect_context::<WorkspaceState>();

    // Drive ChatState from run.* events (single-agent stream only — no team).
    let sub_id = subscribe_run_events(&dashboard, chat, workspace);

    // Ask the Gateway to forward stream.* once connected (poll up to ~5s).
    {
        let dash = dashboard;
        spawn_local(async move {
            for _ in 0..50 {
                if dash.is_connected.get_untracked() {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(100).await;
            }
            if let Err(e) = dash.subscribe_topic("stream.*").await {
                web_sys::console::error_1(&format!("phone chat stream sub failed: {e}").into());
            }
        });
    }

    on_cleanup(move || {
        dashboard.unsubscribe_events(sub_id);
        let dash = dashboard;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
        });
    });

    let location = use_location();
    move || {
        if location.pathname.get() == "/chat" {
            view! { <PhoneChatThread/> }.into_any()
        } else {
            view! { <PhoneChatList/> }.into_any()
        }
    }
}
