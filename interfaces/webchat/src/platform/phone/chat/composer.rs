//! Minimal phone composer: an auto-growing textarea + a send/stop button.
//! Faithful subset of the wide `InputArea::send_message` flow (no attachments,
//! slash-commands, @-mentions, team routing, or model override — server remains
//! the prompt-injection authority). Streaming deltas arrive via the run.* event
//! subscription owned by `PhoneChat`.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::state::{ChatPhase, ChatSendError};
use crate::views::chat::ChatState;

#[component]
#[must_use]
pub fn PhoneComposer() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);

    // True while a run is in flight → button becomes Stop.
    let running = move || {
        matches!(chat.phase.get(), ChatPhase::Thinking | ChatPhase::Streaming)
            || chat.active_run_id.get().is_some()
    };

    let send = move || {
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        is_sending.set(true);
        input_text.set(String::new());
        chat.push_user_message(&text);

        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            let res = ChatApi::send(
                &dash,
                &text,
                session_key.as_deref(),
                Vec::new(),
                agent_id.as_deref(),
                project_root.as_deref(),
                None,
            )
            .await;
            match res {
                Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                Err(e) => chat.set_send_error(ChatSendError::classify(e)),
            }
            is_sending.set(false);
        });
    };

    let stop = move || {
        let Some(run_id) = chat.active_run_id.get_untracked() else { return };
        let dash = dashboard;
        spawn_local(async move {
            let _ = ChatApi::abort(&dash, &run_id).await;
        });
    };

    // Enter sends; Shift+Enter inserts a newline.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            send();
        }
    };

    view! {
        <div style="flex:none; display:flex; align-items:flex-end; gap:8px; padding:8px 12px calc(8px + env(safe-area-inset-bottom)); border-top:1px solid var(--color-border-subtle); background:var(--color-surface);">
            <textarea
                prop:value=move || input_text.get()
                on:input=move |ev| input_text.set(event_target_value(&ev))
                on:keydown=on_keydown
                placeholder="Message…"
                rows="1"
                style="flex:1; resize:none; max-height:140px; min-height:38px; padding:9px 12px; border:1px solid var(--color-border); border-radius:var(--radius-xl); background:var(--color-surface-raised); color:var(--color-text-primary); font:inherit; font-size:15px; outline:none;"
            ></textarea>
            {move || if running() {
                view! {
                    <button
                        on:click=move |_| stop()
                        style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-danger); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                        aria-label="Stop"
                    ><svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg></button>
                }.into_any()
            } else {
                view! {
                    <button
                        on:click=move |_| send()
                        style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-primary); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                        aria-label="Send"
                    ><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"></line><polyline points="5 12 12 5 19 12"></polyline></svg></button>
                }.into_any()
            }}
        </div>
    }
}
