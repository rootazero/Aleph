// apps/panel/src/components/chat_sidebar.rs
//
// Chat mode sidebar — agent dropdown + session list.
// Top dropdown selects agent, list shows that agent's sessions.
// Auto-refreshed via stream.session_updated Gateway events.
//
use leptos::prelude::*;
use serde::Deserialize;
use std::sync::Arc;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::state::ChatState;

/// A session entry returned by the backend (sessions.list).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct SessionEntry {
    key: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    message_count: u32,
    /// Backend sends updated_at as Unix epoch seconds (Option<i64>)
    #[serde(default)]
    updated_at: Option<i64>,
}

/// An agent entry returned by the backend (agents.list).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct AgentEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_default: bool,
}

#[component]
pub fn ChatSidebar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let agents = RwSignal::new(Vec::<AgentEntry>::new());
    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);
    // Which agent is selected in the dropdown (synced with chat.agent_id)
    let selected_agent = RwSignal::new(Option::<String>::None);

    // Reusable closure: fetch both agents and sessions from the backend.
    let reload_data = Arc::new(move |dash: DashboardState| {
        is_loading.set(true);
        leptos::task::spawn_local(async move {
            // Fetch agents
            match dash
                .rpc_call("agents.list", serde_json::json!({}))
                .await
            {
                Ok(result) => {
                    if let Some(arr) = result.get("agents") {
                        if let Ok(list) =
                            serde_json::from_value::<Vec<AgentEntry>>(arr.clone())
                        {
                            // Auto-select default agent if none selected
                            if selected_agent.get_untracked().is_none() {
                                let default_id = list
                                    .iter()
                                    .find(|a| a.is_default)
                                    .or(list.first())
                                    .map(|a| a.id.clone());
                                if let Some(id) = default_id {
                                    selected_agent.set(Some(id.clone()));
                                    chat.agent_id.set(Some(id));
                                }
                            }
                            agents.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to list agents: {e}").into(),
                    );
                }
            }

            // Fetch sessions
            match dash
                .rpc_call("sessions.list", serde_json::json!({}))
                .await
            {
                Ok(result) => {
                    if let Some(arr) = result.get("sessions") {
                        if let Ok(list) =
                            serde_json::from_value::<Vec<SessionEntry>>(arr.clone())
                        {
                            sessions.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to list sessions: {e}").into(),
                    );
                }
            }

            is_loading.set(false);
        });
    });

    // Fetch data on mount when connected
    let dash = dashboard;
    let reload_for_mount = reload_data.clone();
    Effect::new(move || {
        if dash.is_connected.get() {
            reload_for_mount(dash);
        }
    });

    // Subscribe to session_updated events so the list refreshes automatically.
    let reload_for_event = reload_data.clone();
    let sub_dash = dashboard;
    let subscription_id = dashboard.subscribe_events(move |event| {
        if event.topic == "run.session_updated" {
            reload_for_event(sub_dash);
        }
    });

    // Ask the Gateway to push stream.session_updated events to this client.
    let dash_for_topic = dashboard;
    leptos::task::spawn_local(async move {
        if let Err(e) = dash_for_topic
            .subscribe_topic("stream.session_updated")
            .await
        {
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

    // Select a session and load its history.
    let on_select_session = move |key: String, agent_id: String| {
        let dash = dashboard;
        let current = chat.session_key.get_untracked();
        if current.as_deref() == Some(&key) {
            return;
        }
        chat.clear_session();
        chat.agent_id.set(Some(agent_id.clone()));
        selected_agent.set(Some(agent_id));
        chat.session_key.set(Some(key.clone()));

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
                    web_sys::console::error_1(
                        &format!("Failed to load history: {e}").into(),
                    );
                }
            }
        });
    };

    // Handle agent dropdown change
    let on_agent_change = move |ev: web_sys::Event| {
        let val = event_target_value(&ev);
        if !val.is_empty() {
            selected_agent.set(Some(val.clone()));
            chat.agent_id.set(Some(val));
            // Don't clear session — user might just be browsing agents
        }
    };

    // Start a new chat for the selected agent.
    let on_new_chat = move |_: web_sys::MouseEvent| {
        if let Some(agent_id) = selected_agent.get_untracked() {
            chat.clear_session();
            chat.agent_id.set(Some(agent_id));
        }
    };

    view! {
        <div class="flex flex-col h-full">
            // Agent selector + New Chat button
            <div class="p-3 space-y-2">
                <div class="flex items-center gap-2">
                    <select
                        class="flex-1 px-3 py-1.5 rounded-lg bg-surface-sunken border border-border
                               text-sm text-text-primary focus:outline-none focus:ring-2
                               focus:ring-primary/30 focus:border-primary"
                        on:change=on_agent_change
                    >
                        {move || {
                            let agent_list = agents.get();
                            let sel = selected_agent.get();
                            agent_list
                                .into_iter()
                                .map(|agent| {
                                    let id = agent.id.clone();
                                    let name = agent
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| agent.id.clone());
                                    let is_selected = sel.as_deref() == Some(&id);
                                    view! {
                                        <option value={id} selected=is_selected>
                                            {name}
                                        </option>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </select>
                    <button
                        class="px-3 py-1.5 rounded-lg bg-primary text-white text-sm font-medium
                               hover:bg-primary/90 transition-colors whitespace-nowrap"
                        on:click=on_new_chat
                    >
                        "+ New"
                    </button>
                </div>

                // Search
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">"Search chats..."</span>
                </div>
            </div>

            // Session list (filtered by selected agent)
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                {move || {
                    let session_list = sessions.get();
                    let sel_agent = selected_agent.get();
                    let _active_key = chat.session_key.get(); // track for reactivity

                    if is_loading.get() && session_list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Loading..."
                            </p>
                        }.into_any();
                    }

                    // Filter sessions for selected agent, sorted by updated_at desc
                    let mut filtered: Vec<SessionEntry> = session_list
                        .into_iter()
                        .filter(|s| sel_agent.as_deref() == Some(&s.agent_id))
                        .collect();
                    filtered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

                    if filtered.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "No conversations yet"
                            </p>
                        }.into_any();
                    }

                    let on_select = on_select_session.clone();
                    view! {
                        <div class="space-y-0.5">
                            {filtered
                                .into_iter()
                                .map(|session| {
                                    let key = session.key.clone();
                                    let key_for_click = key.clone();
                                    let session_agent_id = session.agent_id.clone();
                                    let is_active = {
                                        let key = key.clone();
                                        move || {
                                            chat.session_key.get().as_deref() == Some(&key)
                                        }
                                    };
                                    let label = session
                                        .topic
                                        .clone()
                                        .unwrap_or_else(|| "New Chat".to_string());
                                    let subtitle = format_session_subtitle(&session);
                                    let on_select = on_select.clone();
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
                                            on:click=move |_| on_select(
                                                key_for_click.clone(),
                                                session_agent_id.clone(),
                                            )
                                        >
                                            <div class="truncate font-medium text-xs">
                                                {label}
                                            </div>
                                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                {subtitle}
                                            </div>
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}

fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    match session.updated_at {
        Some(ts) => {
            // Format Unix epoch seconds as MM-DD using js_sys::Date (WASM-safe)
            let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
            let month = date.get_month() + 1; // 0-based in JS
            let day = date.get_date();
            format!("{msg_count} msgs - {:02}-{:02}", month, day)
        }
        None => format!("{msg_count} messages"),
    }
}
