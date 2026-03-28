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
use crate::i18n::*;
use crate::views::chat::state::ChatState;

use web_sys::HtmlInputElement;

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
    let i18n = use_i18n();

    let agents = RwSignal::new(Vec::<AgentEntry>::new());
    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);
    // Which agent is selected in the dropdown (synced with chat.agent_id)
    let selected_agent = RwSignal::new(Option::<String>::None);

    // Session action states (edit/delete/menu — mutually exclusive)
    let editing_key = RwSignal::new(Option::<String>::None);
    let deleting_key = RwSignal::new(Option::<String>::None);
    let edit_text = RwSignal::new(String::new());
    let menu_open_key = RwSignal::new(Option::<String>::None);
    let is_saving = RwSignal::new(false);
    let edit_input_ref = NodeRef::<leptos::html::Input>::new();

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
                            is_intermediate: false,
                            error: None,
                            model_info: None,
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

    // --- Session action helpers ---

    let clear_action_states = move || {
        editing_key.set(None);
        deleting_key.set(None);
        menu_open_key.set(None);
        edit_text.set(String::new());
    };

    let reload_for_rename = reload_data.clone();
    let do_rename = Arc::new(move |session_key: String, topic: String| {
        if is_saving.get_untracked() {
            return;
        }
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            editing_key.set(None);
            edit_text.set(String::new());
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_rename.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({
                "session_key": session_key,
                "topic": topic,
            });
            match dash.rpc_call("sessions.set_topic", params).await {
                Ok(_) => {
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to rename session: {e}").into(),
                    );
                }
            }
            is_saving.set(false);
            editing_key.set(None);
            edit_text.set(String::new());
        });
    });

    let reload_for_delete = reload_data.clone();
    let do_delete = Arc::new(move |session_key: String| {
        if is_saving.get_untracked() {
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_delete.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({
                "session_key": session_key,
            });
            match dash.rpc_call("sessions.delete", params).await {
                Ok(_) => {
                    // If deleting the active session, clear it
                    if chat.session_key.get_untracked().as_deref() == Some(&session_key) {
                        chat.clear_session();
                    }
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to delete session: {e}").into(),
                    );
                }
            }
            is_saving.set(false);
            deleting_key.set(None);
        });
    });

    // Auto-focus edit input when entering edit mode
    Effect::new(move || {
        let _key = editing_key.get();
        if _key.is_some() {
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(10).await;
                if let Some(el) = edit_input_ref.get() {
                    let input: &HtmlInputElement = &el;
                    let _ = input.focus();
                    input.select();
                }
            });
        }
    });

    // Auto-dismiss delete confirmation after 5 seconds
    Effect::new(move || {
        let key = deleting_key.get();
        if let Some(k) = key {
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(5000).await;
                if deleting_key.get_untracked().as_deref() == Some(&k) {
                    deleting_key.set(None);
                }
            });
        }
    });

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
                        {move || t_string!(i18n, chat.new).to_string()}
                    </button>
                </div>

                // Search
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">{move || t_string!(i18n, chat.search_placeholder).to_string()}</span>
                </div>
            </div>

            // Click-outside overlay for dropdown menu
            {move || {
                if menu_open_key.get().is_some() {
                    view! { <div class="fixed inset-0 z-40" on:click=move |_| menu_open_key.set(None) /> }.into_any()
                } else {
                    view! { <span /> }.into_any()
                }
            }}

            // Session list (filtered by selected agent)
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                {move || {
                    let session_list = sessions.get();
                    let sel_agent = selected_agent.get();
                    let _active_key = chat.session_key.get(); // track for reactivity
                    // Track action states for reactivity
                    let _editing = editing_key.get();
                    let _deleting = deleting_key.get();
                    let _menu = menu_open_key.get();

                    if is_loading.get() && session_list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                {move || t_string!(i18n, common.loading).to_string()}
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
                                {move || t_string!(i18n, chat.no_conversations).to_string()}
                            </p>
                        }.into_any();
                    }

                    let on_select = on_select_session;
                    let do_rename = do_rename.clone();
                    let do_delete = do_delete.clone();
                    view! {
                        <div class="space-y-0.5">
                            {filtered
                                .into_iter()
                                .map(|session| {
                                    let key = session.key.clone();
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
                                        .unwrap_or_else(|| t_string!(i18n, chat.new_chat).to_string());
                                    let subtitle = format_session_subtitle(&session);
                                    let do_rename = do_rename.clone();
                                    let do_delete = do_delete.clone();

                                    // Determine which mode this session row is in
                                    let is_editing = _editing.as_deref() == Some(&key);
                                    let is_deleting = _deleting.as_deref() == Some(&key);
                                    let is_menu_open = _menu.as_deref() == Some(&key);

                                    if is_editing {
                                        // --- Edit mode ---
                                        let key_for_save = key.clone();
                                        let key_for_save2 = key.clone();
                                        let do_rename_keydown = do_rename.clone();
                                        let do_rename_blur = do_rename.clone();
                                        view! {
                                            <div class="w-full px-3 py-2 rounded-lg bg-surface-sunken border border-primary/40">
                                                <input
                                                    node_ref=edit_input_ref
                                                    class="w-full bg-transparent text-xs text-text-primary outline-none disabled:opacity-50"
                                                    prop:value=move || edit_text.get()
                                                    prop:disabled=move || is_saving.get()
                                                    maxlength=100
                                                    on:input=move |ev| {
                                                        edit_text.set(event_target_value(&ev));
                                                    }
                                                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                        let k = ev.key();
                                                        if k == "Enter" {
                                                            let text = edit_text.get_untracked();
                                                            if text.trim().is_empty() {
                                                                editing_key.set(None);
                                                                edit_text.set(String::new());
                                                            } else {
                                                                do_rename_keydown(key_for_save.clone(), text);
                                                            }
                                                        } else if k == "Escape" {
                                                            editing_key.set(None);
                                                            edit_text.set(String::new());
                                                        }
                                                    }
                                                    on:blur=move |_| {
                                                        // Small delay to allow Enter keydown to fire first
                                                        let key_c = key_for_save2.clone();
                                                        let do_rename_c = do_rename_blur.clone();
                                                        leptos::task::spawn_local(async move {
                                                            gloo_timers::future::TimeoutFuture::new(100).await;
                                                            if editing_key.get_untracked().as_deref() == Some(&key_c) {
                                                                let text = edit_text.get_untracked();
                                                                if text.trim().is_empty() {
                                                                    editing_key.set(None);
                                                                    edit_text.set(String::new());
                                                                } else {
                                                                    do_rename_c(key_c, text);
                                                                }
                                                            }
                                                        });
                                                    }
                                                />
                                            </div>
                                        }.into_any()
                                    } else if is_deleting {
                                        // --- Delete-confirm mode ---
                                        let key_for_del = key.clone();
                                        view! {
                                            <div
                                                tabindex=0
                                                class="w-full px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30
                                                        flex items-center justify-between text-xs outline-none"
                                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                    if ev.key() == "Escape" {
                                                        clear_action_states();
                                                    }
                                                }
                                            >
                                                <span class="text-red-400 font-medium">{move || t_string!(i18n, chat.confirm_delete).to_string()}</span>
                                                <div class="flex items-center gap-1.5">
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-red-500 text-white text-[10px] font-medium
                                                               hover:bg-red-600 transition-colors disabled:opacity-50"
                                                        prop:disabled=move || is_saving.get()
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            do_delete(key_for_del.clone());
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.confirm).to_string()}
                                                    </button>
                                                    <button
                                                        class="px-2 py-0.5 rounded bg-surface-sunken text-text-secondary text-[10px]
                                                               hover:bg-surface-raised transition-colors"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            clear_action_states();
                                                        }
                                                    >
                                                        {move || t_string!(i18n, common.cancel).to_string()}
                                                    </button>
                                                </div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        // --- Normal mode ---
                                        let key_for_click = key.clone();
                                        let key_for_menu = key.clone();
                                        let key_for_edit = key.clone();
                                        let key_for_del_menu = key.clone();
                                        let label_for_edit = label.clone();
                                        view! {
                                            <div class="relative group">
                                                <button
                                                    class=move || format!(
                                                        "w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors flex items-center justify-between {}",
                                                        if is_active() {
                                                            "bg-primary/10 text-primary font-medium"
                                                        } else {
                                                            "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                                                        }
                                                    )
                                                    on:click=move |_| {
                                                        clear_action_states();
                                                        on_select(
                                                            key_for_click.clone(),
                                                            session_agent_id.clone(),
                                                        );
                                                    }
                                                >
                                                    <div class="flex-1 min-w-0">
                                                        <div class="truncate font-medium text-xs">
                                                            {label}
                                                        </div>
                                                        <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                            {subtitle}
                                                        </div>
                                                    </div>
                                                    // ⋯ button (visible on hover)
                                                    <button
                                                        class="opacity-0 group-hover:opacity-100 ml-1 px-1.5 py-0.5
                                                               rounded text-text-tertiary hover:text-text-primary
                                                               hover:bg-surface-raised transition-all text-xs flex-shrink-0"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            let current = menu_open_key.get_untracked();
                                                            if current.as_deref() == Some(&key_for_menu) {
                                                                menu_open_key.set(None);
                                                            } else {
                                                                clear_action_states();
                                                                menu_open_key.set(Some(key_for_menu.clone()));
                                                            }
                                                        }
                                                    >
                                                        "⋯"
                                                    </button>
                                                </button>
                                                // Dropdown menu
                                                {if is_menu_open {
                                                    view! {
                                                        <div class="absolute right-0 top-full mt-1 z-50 min-w-[120px]
                                                                    bg-surface-raised border border-border rounded-lg shadow-lg
                                                                    py-1 text-xs">
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-text-secondary
                                                                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    menu_open_key.set(None);
                                                                    edit_text.set(label_for_edit.clone());
                                                                    editing_key.set(Some(key_for_edit.clone()));
                                                                }
                                                            >
                                                                {move || t_string!(i18n, chat.rename).to_string()}
                                                            </button>
                                                            <button
                                                                class="w-full text-left px-3 py-1.5 text-red-400
                                                                       hover:bg-red-500/10 transition-colors"
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    menu_open_key.set(None);
                                                                    deleting_key.set(Some(key_for_del_menu.clone()));
                                                                }
                                                            >
                                                                {move || t_string!(i18n, common.delete).to_string()}
                                                            </button>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! { <span /> }.into_any()
                                                }}
                                            </div>
                                        }.into_any()
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
