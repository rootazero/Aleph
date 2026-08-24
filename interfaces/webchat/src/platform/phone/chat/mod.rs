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
use crate::i18n::{t_string, use_i18n};
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;

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
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();

    // Register this surface's conversation in `SessionMap`.
    //
    // Subscribing to `stream.*` was only ever half of what it takes to see a
    // turn. The other half is having a `ConvId` for `resolve_target` to resolve
    // to: its three steps are route → frame's own `session_key` → foreground,
    // and on phone all three were structurally empty because `ChatSidebar` —
    // the only thing in the crate that ever opened a conversation — is mounted
    // behind `not_phone`. Every `run_accepted` therefore returned `None` and
    // the handler returned before touching anything, so a phone (and the iOS
    // Panel shell, which is always in the phone band) rendered no assistant
    // bubble, no tool rows and no final answer — the turn only appeared after
    // leaving the surface and coming back, and nothing was logged anywhere.
    //
    // Done at mount rather than lazily on the first send: `activate` restores
    // the incoming conversation's (empty) snapshot into the singleton, so
    // creating the conversation after the optimistic user bubble is pushed
    // would erase it. At mount the singleton is either untouched (cold boot) or
    // the conversation already exists and this is a no-op.
    sessions.ensure_active(chat, &chat.agent_id.get_untracked().unwrap_or_default(), || {
        t_string!(i18n, chat.new_chat).to_string()
    });

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

#[cfg(test)]
mod tests {
    /// The phone chat router must register a conversation.
    ///
    /// Source-level because the property is about a component body no headless
    /// test mounts, and because the failure it pins is invisible at runtime: a
    /// surface with no `ConvId` still connects, still subscribes, still sends,
    /// still shows the user's own bubble — it just never receives a frame,
    /// because `resolve_target` has nothing to resolve to (see
    /// `a_surface_that_registers_no_conversation_receives_no_frame`).
    ///
    /// The wide half needs no twin: `ChatSidebar` opens conversations from
    /// three gestures and its absence is what this file compensates for.
    #[test]
    fn the_phone_chat_router_registers_a_conversation() {
        let src = include_str!("mod.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            production.contains("ensure_active("),
            "PhoneChat no longer registers a conversation — every stream frame \
             this surface receives will be dropped, silently"
        );
    }
}
