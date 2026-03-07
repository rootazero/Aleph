// apps/panel/src/components/chat_sidebar.rs
//
// Chat mode sidebar — session list fetched from sessions.list RPC,
// auto-refreshed via stream.session_updated Gateway events.
//
use leptos::prelude::*;
use serde::Deserialize;
use std::sync::Arc;
use crate::context::DashboardState;
use crate::views::chat::state::ChatState;
use crate::api::chat::ChatApi;

/// A session entry returned by the backend.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionEntry {
    key: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    last_active_at: String,
}

#[component]
pub fn ChatSidebar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);

    // Reusable closure: fetch sessions from the backend and update the signal.
    // Wrapped in Arc so it can be shared between the initial Effect and the event handler.
    let reload_sessions = Arc::new(move |dash: DashboardState| {
        is_loading.set(true);
        leptos::task::spawn_local(async move {
            match dash.rpc_call("sessions.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("sessions") {
                        if let Ok(list) = serde_json::from_value::<Vec<SessionEntry>>(arr.clone()) {
                            sessions.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list sessions: {e}").into());
                }
            }
            is_loading.set(false);
        });
    });

    // Fetch sessions on mount when connected
    let dash = dashboard;
    let reload_for_mount = reload_sessions.clone();
    Effect::new(move || {
        if dash.is_connected.get() {
            reload_for_mount(dash);
        }
    });

    // Subscribe to session_updated events so the list refreshes automatically.
    // The Gateway emits stream.session_updated which the message loop converts
    // to GatewayEvent { topic: "run.session_updated", .. }.
    let reload_for_event = reload_sessions.clone();
    let sub_dash = dashboard;
    let subscription_id = dashboard.subscribe_events(move |event| {
        if event.topic == "run.session_updated" {
            reload_for_event(sub_dash);
        }
    });

    // Ask the Gateway to push stream.session_updated events to this client.
    // (The chat view already subscribes to stream.* but we do it explicitly
    // for robustness in case the sidebar mounts independently.)
    let dash_for_topic = dashboard;
    leptos::task::spawn_local(async move {
        if let Err(e) = dash_for_topic.subscribe_topic("stream.session_updated").await {
            web_sys::console::error_1(
                &format!("Failed to subscribe to stream.session_updated: {e}").into(),
            );
        }
    });

    // Cleanup: unsubscribe event handler when the component unmounts.
    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(subscription_id);
    });

    let on_select_session = move |key: String| {
        let dash = dashboard;
        let current = chat.session_key.get_untracked();
        if current.as_deref() == Some(&key) {
            return; // already selected
        }
        // Clear current messages and set new session key
        chat.clear();
        chat.session_key.set(Some(key.clone()));

        // Load history for selected session
        leptos::task::spawn_local(async move {
            match ChatApi::history(&dash, &key, Some(50)).await {
                Ok(history) => {
                    let msgs: Vec<crate::views::chat::state::ChatMessage> = history
                        .into_iter()
                        .enumerate()
                        .map(|(i, m)| crate::views::chat::state::ChatMessage {
                            id: m.run_id.unwrap_or_else(|| format!("hist-{i}")),
                            role: m.role,
                            content: m.content,
                            tool_calls: vec![],
                            is_streaming: false,
                            error: None,
                        })
                        .collect();
                    chat.messages.set(msgs);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load history: {e}").into());
                }
            }
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Search
            <div class="p-3">
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">"Search chats..."</span>
                </div>
            </div>

            // Session list
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                {move || {
                    let list = sessions.get();
                    let _active_key = chat.session_key.get();
                    if is_loading.get() {
                        view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Loading sessions..."
                            </p>
                        }.into_any()
                    } else if list.is_empty() {
                        view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Start a new conversation"
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <For
                                each=move || sessions.get()
                                key=|s| s.key.clone()
                                children=move |session| {
                                    let key = session.key.clone();
                                    let key_for_click = key.clone();
                                    let is_active = move || {
                                        chat.session_key.get().as_deref() == Some(&key)
                                    };
                                    let on_select = on_select_session.clone();
                                    let label = format_session_label(&session);
                                    let subtitle = format_session_subtitle(&session);
                                    view! {
                                        <button
                                            class=move || format!(
                                                "w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors {}",
                                                if is_active() {
                                                    "bg-primary/10 text-primary font-medium"
                                                } else {
                                                    "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                                                }
                                            )
                                            on:click=move |_| on_select(key_for_click.clone())
                                        >
                                            <div class="truncate font-medium text-xs">{label}</div>
                                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">{subtitle}</div>
                                        </button>
                                    }
                                }
                            />
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

fn format_session_label(session: &SessionEntry) -> String {
    // Extract meaningful part from session key like "agent:main:main" -> "main"
    let parts: Vec<&str> = session.key.split(':').collect();
    if parts.len() >= 3 {
        format!("{} ({})", parts[1], parts[2])
    } else {
        session.key.clone()
    }
}

fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    if session.last_active_at.is_empty() {
        format!("{msg_count} messages")
    } else {
        // Show short date from ISO 8601
        let date = session.last_active_at
            .split('T')
            .next()
            .unwrap_or(&session.last_active_at);
        format!("{msg_count} msgs - {date}")
    }
}
