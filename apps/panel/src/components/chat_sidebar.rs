// apps/panel/src/components/chat_sidebar.rs
//
// Chat mode sidebar — two-level agent/session hierarchy.
// Fetches agents from agents.list and sessions from sessions.list,
// auto-refreshed via stream.session_updated Gateway events.
//
use leptos::prelude::*;
use serde::Deserialize;
use std::collections::HashSet;
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
    let collapsed = RwSignal::new(HashSet::<String>::new());

    // Reusable closure: fetch both agents and sessions from the backend.
    // Wrapped in Arc so it can be shared between the initial Effect and the event handler.
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
    // The Gateway emits stream.session_updated which the message loop converts
    // to GatewayEvent { topic: "run.session_updated", .. }.
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
        chat.agent_id.set(Some(agent_id));
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

    // Start a new chat for an agent.
    let on_new_chat = move |agent_id: String| {
        chat.clear_session();
        chat.agent_id.set(Some(agent_id));
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

            // Agent/session list
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                {move || {
                    let agent_list = agents.get();
                    let session_list = sessions.get();
                    let _active_key = chat.session_key.get();
                    let _active_agent = chat.agent_id.get();

                    if is_loading.get() && agent_list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Loading..."
                            </p>
                        }.into_any();
                    }

                    if agent_list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "No agents configured"
                            </p>
                        }.into_any();
                    }

                    // Build agent groups
                    let groups: Vec<(AgentEntry, Vec<SessionEntry>)> = agent_list
                        .iter()
                        .map(|agent| {
                            let mut agent_sessions: Vec<SessionEntry> = session_list
                                .iter()
                                .filter(|s| s.agent_id == agent.id)
                                .cloned()
                                .collect();
                            // Sort by updated_at descending (most recent first)
                            agent_sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                            (agent.clone(), agent_sessions)
                        })
                        .collect();

                    view! {
                        <div class="space-y-1">
                            {groups
                                .into_iter()
                                .map(|(agent, agent_sessions)| {
                                    let agent_id = agent.id.clone();
                                    let agent_id_for_toggle = agent_id.clone();
                                    let agent_id_for_new = agent_id.clone();
                                    let agent_name = agent
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| agent.id.clone());
                                    let session_count = agent_sessions.len();

                                    // Auto-expand active agent's group
                                    let agent_id_for_check = agent_id.clone();
                                    let agent_id_for_check2 = agent_id.clone();
                                    let is_collapsed_icon = move || {
                                        let c = collapsed.get();
                                        let active = chat.agent_id.get();
                                        if active.as_deref() == Some(&agent_id_for_check) && !c.contains(&agent_id_for_check) {
                                            return false;
                                        }
                                        c.contains(&agent_id_for_check)
                                    };
                                    let is_collapsed_body = move || {
                                        let c = collapsed.get();
                                        let active = chat.agent_id.get();
                                        if active.as_deref() == Some(&agent_id_for_check2) && !c.contains(&agent_id_for_check2) {
                                            return false;
                                        }
                                        c.contains(&agent_id_for_check2)
                                    };

                                    let toggle_collapse = move |_: web_sys::MouseEvent| {
                                        collapsed.update(|set| {
                                            if set.contains(&agent_id_for_toggle) {
                                                set.remove(&agent_id_for_toggle);
                                            } else {
                                                set.insert(agent_id_for_toggle.clone());
                                            }
                                        });
                                    };

                                    let on_new = on_new_chat.clone();
                                    let new_chat_click = move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        on_new(agent_id_for_new.clone());
                                    };

                                    let on_select = on_select_session.clone();

                                    view! {
                                        <div class="mb-2">
                                            // Agent header
                                            <button
                                                class="w-full flex items-center gap-1.5 px-2 py-1.5 rounded-md text-xs font-semibold text-text-secondary hover:bg-surface-sunken transition-colors group"
                                                on:click=toggle_collapse
                                            >
                                                <span class="text-text-tertiary text-[10px] w-3">
                                                    {move || if is_collapsed_icon() { "\u{25B6}" } else { "\u{25BC}" }}
                                                </span>
                                                <span class="truncate flex-1 text-left">{agent_name}</span>
                                                <span class="text-text-tertiary text-[10px]">
                                                    {session_count}
                                                </span>
                                                <button
                                                    class="opacity-0 group-hover:opacity-100 text-text-tertiary hover:text-text-primary transition-opacity px-1"
                                                    on:click=new_chat_click
                                                    title="New chat"
                                                >
                                                    "+"
                                                </button>
                                            </button>

                                            // Session list (collapsible)
                                            <Show when=move || !is_collapsed_body()>
                                                <div class="ml-3 mt-0.5 space-y-0.5">
                                                    {agent_sessions
                                                        .clone()
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
                                                                        "w-full text-left px-3 py-2 rounded-lg text-sm transition-colors {}",
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
                                            </Show>
                                        </div>
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
