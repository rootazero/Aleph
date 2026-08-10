//! Native iPhone Chat screens (single-agent). The chat surface is the tab
//! landing (`/`); the session history is reached via the surface's history
//! button (`/chat/history`). Reuses ChatState / ChatApi / MessageList; only the
//! history list and a minimal composer are phone-specific.

pub mod composer;
pub mod history;
pub mod thread;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::context::DashboardState;

use self::history::PhoneChatHistory;
use self::thread::PhoneChatThread;

/// Phone Chat router. Owns the `run.*` streaming subscription (mirrors the wide
/// `ChatView`); exactly one of {ChatView, PhoneChat} mounts per form factor, so
/// there is no double-subscribe. Renders the chat surface at `/` and the
/// session history at `/chat/history`.
#[component]
#[must_use]
pub fn PhoneChat() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();

    // Ask the Gateway to forward stream.* once connected (poll up to ~5s).
    {
        let dash = dashboard;
        spawn_local(async move {
            // The socket wait lives in `DashboardState::rpc_call` now; the
            // 50×100 ms poll this used to open with is gone.
            if let Err(e) = dash.subscribe_topic("stream.*").await {
                web_sys::console::error_1(&format!("phone chat stream sub failed: {e}").into());
            }
            // `stream.ask_user` is a one-shot push: seed from the registry so a
            // surface that connects mid-question still finds the parked tool.
            match crate::api::ClarificationApi::list_pending(&dash).await {
                Ok(list) => dash.pending_clarifications.set(list),
                Err(e) => web_sys::console::error_1(
                    &format!("phone chat pending clarifications failed: {e}").into(),
                ),
            }
        });
    }

    on_cleanup(move || {
        let dash = dashboard;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
        });
    });

    let location = use_location();
    move || {
        if location.pathname.get() == "/chat/history" {
            view! { <PhoneChatHistory/> }.into_any()
        } else {
            view! { <PhoneChatThread/> }.into_any()
        }
    }
}
