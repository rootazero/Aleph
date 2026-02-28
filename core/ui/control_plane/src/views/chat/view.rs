// core/ui/control_plane/src/views/chat/view.rs
//! Main Chat view — message list + input area.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::chat::ChatApi;
use super::state::{ChatState, ChatPhase, ChatMessage};
use super::events::subscribe_run_events;

/// Top-level Chat view component.
#[component]
pub fn ChatView() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = ChatState::new();
    provide_context(chat);

    // Subscribe to run.* events directly (no Effect — this is a one-shot mount action)
    let sub_id = subscribe_run_events(&dashboard, chat);

    // Tell the Gateway to start forwarding run.* events
    spawn_local(async move {
        let dashboard = expect_context::<DashboardState>();
        if let Err(e) = dashboard.subscribe_topic("run.*").await {
            web_sys::console::error_1(&format!("Failed to subscribe run events: {e}").into());
        }
    });

    on_cleanup(move || {
        dashboard.unsubscribe_events(sub_id);
        // Tell the Gateway to stop forwarding run.* events
        spawn_local(async move {
            let dashboard = expect_context::<DashboardState>();
            let _ = dashboard.unsubscribe_topic("run.*").await;
        });
    });

    view! {
        <div class="flex flex-col h-full bg-surface">
            // Message list (scrollable)
            <MessageList />
            // Input area (pinned to bottom)
            <InputArea />
        </div>
    }
}

/// Scrollable message list.
#[component]
fn MessageList() -> impl IntoView {
    let chat = expect_context::<ChatState>();

    view! {
        <div class="flex-1 overflow-y-auto px-4 py-6 space-y-4">
            <For
                each=move || chat.messages.get()
                key=|msg| msg.id.clone()
                children=move |msg| {
                    view! { <MessageBubble message=msg /> }
                }
            />
            // Thinking indicator
            <Show when=move || chat.phase.get() == ChatPhase::Thinking>
                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                    <span class="inline-block w-2 h-2 rounded-full bg-primary animate-pulse"></span>
                    "Thinking..."
                </div>
            </Show>
        </div>
    }
}

/// Single message bubble.
#[component]
fn MessageBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    let bubble_align = if is_user { "flex justify-end" } else { "flex justify-start" };
    let bubble_style = if is_user {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-white"
    } else if has_error {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-danger-subtle text-danger border border-danger/20"
    } else {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-surface-raised text-text-primary"
    };

    let tool_calls_view = if has_tools {
        let tools = message.tool_calls.clone();
        Some(view! {
            <div class="mb-2 space-y-1">
                {tools.into_iter().map(|tc| {
                    let status_color = match tc.status.as_str() {
                        "running" => "text-warning",
                        "completed" => "text-success",
                        "failed" => "text-danger",
                        _ => "text-text-secondary",
                    };
                    let status_icon = match tc.status.as_str() {
                        "running" => "\u{27F3}",
                        "completed" => "\u{2713}",
                        "failed" => "\u{2717}",
                        _ => "\u{00B7}",
                    };
                    let duration_text = tc.duration_ms
                        .map(|d| format!(" ({d}ms)"))
                        .unwrap_or_default();
                    view! {
                        <div class="flex items-center gap-2 text-xs font-mono">
                            <span class=status_color>
                                {status_icon}
                            </span>
                            <span class="text-text-secondary">{tc.tool_name.clone()}</span>
                            <span class="text-text-tertiary">{duration_text}</span>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
    } else {
        None
    };

    let content = message.content.clone();
    let is_streaming = message.is_streaming;
    let error = message.error.clone();

    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
        })
    } else {
        None
    };

    let error_view = error.map(|err| view! {
        <div class="mt-2 text-xs text-danger/80">{err}</div>
    });

    view! {
        <div class=bubble_align>
            <div class=bubble_style>
                {tool_calls_view}

                // Message content
                <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                    {content}
                </div>

                // Streaming cursor
                {streaming_cursor}

                // Error message
                {error_view}
            </div>
        </div>
    }
}

/// Text input area with send button.
#[component]
fn InputArea() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);

    let send_message = move || {
        if is_sending.get_untracked() { return; }
        let text = input_text.get_untracked().trim().to_string();
        if text.is_empty() { return; }

        is_sending.set(true);
        input_text.set(String::new());
        chat.push_user_message(&text);

        let session_key = chat.session_key.get();
        spawn_local(async move {
            let dashboard = expect_context::<DashboardState>();
            let chat = expect_context::<ChatState>();
            let sk = session_key.as_deref();
            match ChatApi::send(&dashboard, &text, sk).await {
                Ok(resp) => {
                    chat.session_key.set(Some(resp.session_key));
                    // run_accepted event will trigger start_assistant_message
                }
                Err(e) => {
                    chat.error_message.set(Some(e.clone()));
                    chat.phase.set(ChatPhase::Error);
                }
            }
            is_sending.set(false);
        });
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            send_message();
        }
    };

    let can_send = Memo::new(move |_| {
        !input_text.get().trim().is_empty() && !is_sending.get()
    });

    // Abort button handler
    let on_abort = move |_: web_sys::MouseEvent| {
        if let Some(run_id) = chat.active_run_id.get() {
            spawn_local(async move {
                let dashboard = expect_context::<DashboardState>();
                let _ = ChatApi::abort(&dashboard, &run_id).await;
            });
        }
    };

    view! {
        <div class="border-t border-border px-4 py-3">
            <div class="flex items-end gap-2">
                <textarea
                    class="flex-1 resize-none rounded-xl border border-border bg-surface-sunken px-4 py-2.5
                           text-sm text-text-primary placeholder:text-text-tertiary
                           focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary
                           min-h-[40px] max-h-[120px]"
                    placeholder="Send a message..."
                    rows=1
                    prop:value=move || input_text.get()
                    on:input=move |ev| {
                        input_text.set(event_target_value(&ev));
                    }
                    on:keydown=on_keydown
                />

                // Abort button (when running)
                <Show when=move || chat.active_run_id.get().is_some()>
                    <button
                        class="p-2.5 rounded-xl bg-danger/10 text-danger hover:bg-danger/20 transition-colors"
                        title="Stop"
                        on:click=on_abort
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                            <rect x="4" y="4" width="12" height="12" rx="2" />
                        </svg>
                    </button>
                </Show>

                // Send button (when idle)
                <Show when=move || chat.active_run_id.get().is_none()>
                    <button
                        class="p-2.5 rounded-xl bg-primary text-white hover:bg-primary/90
                               disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                        disabled=move || !can_send.get()
                        on:click=move |_| send_message()
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                            <path d="M3.105 2.288a.75.75 0 0 0-.826.95l1.414 4.926A1.5 1.5 0 0 0 5.135 9.25h6.115a.75.75 0 0 1 0 1.5H5.135a1.5 1.5 0 0 0-1.442 1.086l-1.414 4.926a.75.75 0 0 0 .826.95l14.095-5.61a.75.75 0 0 0 0-1.394L3.105 2.288Z" />
                        </svg>
                    </button>
                </Show>
            </div>
        </div>
    }
}
