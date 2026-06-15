//
// Chat mode sidebar — agent dropdown + session list.
// Top dropdown selects agent, list shows that agent's sessions.
// Auto-refreshed via stream.session_updated Gateway events.
//
use leptos::prelude::*;
use serde::Deserialize;
use std::rc::Rc;
use std::sync::Arc;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
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
    /// Backend sends `updated_at` as Unix epoch seconds (Option<i64>)
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    project_root: Option<String>,
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

/// Fetch a session's history (+ persisted run traces) and rebuild the
/// transcript in `chat.messages`. Shared by session selection and the
/// external-update live refresh (`run.session_updated` with an
/// `origin_channel`); callers must already have `chat.session_key`
/// pointing at `key`.
async fn hydrate_session_history(
    dash: DashboardState,
    chat: ChatState,
    workspace: Option<WorkspaceState>,
    key: String,
) {
    match ChatApi::history(&dash, &key, Some(50)).await {
        Ok(history) => {
            // Distinct assistant run_ids → fetch their persisted traces.
            let run_ids: Vec<String> = {
                let mut seen = std::collections::HashSet::new();
                history
                    .iter()
                    .filter(|m| m.role == "assistant")
                    .filter_map(|m| m.run_id.clone())
                    .filter(|r| seen.insert(r.clone()))
                    .collect()
            };

            let traces: std::collections::HashMap<String, Vec<serde_json::Value>> =
                if run_ids.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    match crate::api::trace::TraceApi::by_runs(&dash, run_ids).await {
                        Ok(runs) => runs,
                        Err(e) => {
                            web_sys::console::warn_1(&format!("trace.by_runs failed: {e}").into());
                            std::collections::HashMap::new()
                        }
                    }
                };

            // Build the transcript in order: replay traced assistant
            // runs into the (already-cleared) real chat; push user rows
            // and trace-less assistant rows as plain bubbles.
            chat.messages.set(Vec::new());
            for (i, m) in history.iter().enumerate() {
                let ts = m
                    .timestamp
                    .as_deref()
                    .and_then(crate::views::chat::timeline::parse_wire_timestamp);

                let traced = m.role == "assistant"
                    && m.run_id
                        .as_deref()
                        .and_then(|r| traces.get(r))
                        .map(|evs| !evs.is_empty())
                        .unwrap_or(false);

                let replayed = if traced {
                    if let (Some(run), Some(ws)) = (m.run_id.as_deref(), workspace) {
                        let evs = traces.get(run).cloned().unwrap_or_default();
                        crate::views::chat::events::replay_run(chat, ws, run, &evs, &m.content);
                        // Stamp the final bubble's timestamp from history
                        // so day separators stay correct.
                        let target = format!("assistant-{run}");
                        chat.messages.update(|msgs| {
                            if let Some(b) = msgs.iter_mut().rev().find(|b| b.id == target) {
                                b.timestamp = ts;
                            }
                        });
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };

                // Fall back to a plain bubble whenever replay did NOT
                // run — including the unreachable "traced but no
                // workspace" case, so a row is never silently dropped.
                if !replayed {
                    chat.messages.update(|msgs| {
                        msgs.push(crate::views::chat::state::ChatMessage {
                            timestamp: ts,
                            id: m.run_id.clone().unwrap_or_else(|| format!("hist-{i}")),
                            role: m.role.clone(),
                            content: m.content.clone(),
                            tool_calls: vec![],
                            is_streaming: false,
                            is_intermediate: false,
                            error: None,
                            model_info: None,
                            iteration: None,
                            is_final: false,
                            text_finalized: false,
                        });
                    });
                }
            }

            // Loading an existing session = all activity already "seen";
            // clear the live-only badge + active-iteration marker that
            // replay set.
            if let Some(ws) = workspace {
                ws.unseen_activity.set(0);
                ws.current_iteration.set(None);
            }
        }
        Err(e) => {
            web_sys::console::error_1(&format!("Failed to load history: {e}").into());
        }
    }
}

#[component]
#[must_use]
pub fn ChatSidebar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let session_map = expect_context::<SessionMap>();
    // Workspace pane state — used to reset the tool-detail view and evict
    // captured tool payloads whenever the chat session changes (switch /
    // new / delete). `Option` + `Copy` so it can be captured into every
    // session-gesture closure without panicking if the pane isn't mounted.
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);

    // Session action states (edit/delete/menu — mutually exclusive)
    let editing_key = RwSignal::new(Option::<String>::None);
    let deleting_key = RwSignal::new(Option::<String>::None);
    let edit_text = RwSignal::new(String::new());
    let menu_open_key = RwSignal::new(Option::<String>::None);
    let is_saving = RwSignal::new(false);
    let edit_input_ref = NodeRef::<leptos::html::Input>::new();

    // Client-side session filter (R4 pure I/O — no backend search).
    let search_query = RwSignal::new(String::new());

    // Collapsible-section UI state (in-memory; persisted preference out of scope).
    // Projects defaults collapsed to avoid visual duplication with Recent —
    // the same non-pinned sessions are shown two ways.
    let show_pinned = RwSignal::new(true);
    let show_recent = RwSignal::new(true);
    let show_projects = RwSignal::new(false);

    // Live-only "session is running" tracking, driven by run lifecycle
    // events. `running` is a refcount per session_key (handles concurrent
    // runs); `run_to_session` maps run_id → session_key because the
    // run_complete / run_error frames carry only run_id.
    let running = RwSignal::new(std::collections::HashMap::<String, usize>::new());
    let run_to_session = RwSignal::new(std::collections::HashMap::<String, String>::new());

    // Reusable closure: fetch agents (for default-agent bootstrap) and
    // sessions from the backend. The persistent agent switcher lives in
    // `SidebarFooter`; here we only consult `agents.list` to auto-open the
    // default agent's tab on first connect.
    let reload_data = Arc::new(move |dash: DashboardState| {
        is_loading.set(true);
        leptos::task::spawn_local(async move {
            // Fetch agents — used only to auto-select the default agent when
            // none is active yet. Routing through SessionMap.activate opens
            // the first tab (and stamps `chat.agent_id`); Cmd+1 focuses it.
            match dash.rpc_call("agents.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("agents") {
                        if let Ok(list) = serde_json::from_value::<Vec<AgentEntry>>(arr.clone()) {
                            if chat.agent_id.get_untracked().is_none() {
                                let default_id = list
                                    .iter()
                                    .find(|a| a.is_default)
                                    .or(list.first())
                                    .map(|a| a.id.clone());
                                if let Some(id) = default_id {
                                    session_map.activate(chat, &id);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list agents: {e}").into());
                }
            }

            // Fetch sessions
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

    // Fetch data on mount when connected
    let dash = dashboard;
    let reload_for_mount = reload_data.clone();
    Effect::new(move || {
        if dash.is_connected.get() {
            reload_for_mount(dash);
        }
    });

    // Subscribe to session_updated events so the list refreshes automatically.
    // Frames carrying an `origin_channel` mean another surface (Telegram,
    // Slack, …) touched the session: if it's the one currently open and no
    // local run is in flight, re-hydrate the transcript so the Panel mirrors
    // the channel conversation live. Panel-originated runs publish no origin
    // and never trigger a self-refresh (no clobbering of streaming state).
    let reload_for_event = reload_data.clone();
    let sub_dash = dashboard;
    let subscription_id = dashboard.subscribe_events(move |event| {
        if event.topic != "run.session_updated" {
            return;
        }
        reload_for_event(sub_dash);

        let origin = event
            .data
            .get("origin_channel")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if origin.is_empty() {
            return;
        }
        let Some(sk) = event.data.get("session_key").and_then(|v| v.as_str()) else {
            return;
        };
        if chat.session_key.get_untracked().as_deref() != Some(sk) {
            return;
        }
        if running.with_untracked(|m| m.contains_key(sk)) {
            return;
        }
        leptos::task::spawn_local(hydrate_session_history(
            sub_dash,
            chat,
            workspace,
            sk.to_string(),
        ));
    });

    // Subscribe to run lifecycle so each session row can show a live
    // "running" dot. Refcounted: a session is "running" while it has ≥1
    // in-flight run. run_complete / run_error carry only run_id, so we
    // resolve the owning session via `run_to_session`.
    let run_subscription_id = dashboard.subscribe_events(move |event| {
        if !event.topic.starts_with("run.") {
            return;
        }
        let data = &event.data;
        let event_type = data
            .get("type")
            .and_then(|t| t.as_str())
            .or_else(|| event.topic.strip_prefix("run."))
            .unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");
        if run_id.is_empty() {
            return;
        }
        match event_type {
            "run_accepted" => {
                let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) else {
                    return;
                };
                run_to_session.update(|m| {
                    m.insert(run_id.to_string(), sk.to_string());
                });
                running.update(|m| {
                    *m.entry(sk.to_string()).or_insert(0) += 1;
                });
            }
            "run_complete" | "run_error" => {
                let sk = run_to_session.with_untracked(|m| m.get(run_id).cloned());
                run_to_session.update(|m| {
                    m.remove(run_id);
                });
                if let Some(sk) = sk {
                    running.update(|m| {
                        if let Some(n) = m.get_mut(&sk) {
                            *n = n.saturating_sub(1);
                            if *n == 0 {
                                m.remove(&sk);
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    });

    // Ask the Gateway to push stream.session_updated events to this client.
    let dash_for_topic = dashboard;
    leptos::task::spawn_local(async move {
        // Wait until connected before subscribing to avoid "Not connected" errors.
        for _ in 0..50 {
            if dash_for_topic.is_connected.get_untracked() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }

        if let Err(e) = dash_for_topic
            .subscribe_topic("stream.session_updated")
            .await
        {
            web_sys::console::error_1(
                &format!("Failed to subscribe to stream.session_updated: {e}").into(),
            );
        }

        // Run lifecycle topics drive the per-session running dot.
        for topic in [
            "stream.run_accepted",
            "stream.run_complete",
            "stream.run_error",
        ] {
            if let Err(e) = dash_for_topic.subscribe_topic(topic).await {
                web_sys::console::error_1(&format!("Failed to subscribe to {topic}: {e}").into());
            }
        }
    });

    // Cleanup: unsubscribe event handler when the component unmounts.
    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(subscription_id);
        dash_for_cleanup.unsubscribe_events(run_subscription_id);
    });

    // Select a session and load its history.
    let on_select_session = move |key: String, agent_id: String| {
        let dash = dashboard;
        let current = chat.session_key.get_untracked();
        if current.as_deref() == Some(&key) {
            return;
        }
        // Switch tabs first (snapshots outgoing, restores agent's tab),
        // then clear that tab's session so the upcoming history load
        // overwrites cleanly without leaking the previous topic.
        session_map.activate(chat, &agent_id);
        chat.clear_session();
        if let Some(ws) = workspace {
            ws.reset();
        }
        // `session_map.activate` above already stamped `chat.agent_id` from
        // the restored snapshot — no separate selection signal to update.
        chat.session_key.set(Some(key.clone()));

        leptos::task::spawn_local(hydrate_session_history(dash, chat, workspace, key));
    };

    // Start a new chat for the selected agent.
    // Just clear UI state — the backend will create a new epoch session
    // when the first message is sent (session_key=None triggers next epoch).
    let on_new_chat = move |_: web_sys::MouseEvent| {
        if let Some(agent_id) = chat.agent_id.get_untracked() {
            chat.clear_session();
            if let Some(ws) = workspace {
                ws.reset();
            }
            chat.agent_id.set(Some(agent_id));
        }
    };

    // --- Session action helpers ---

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
                    web_sys::console::error_1(&format!("Failed to rename session: {e}").into());
                }
            }
            is_saving.set(false);
            editing_key.set(None);
            edit_text.set(String::new());
        });
    });

    let reload_for_pin = reload_data.clone();
    let do_pin = Arc::new(move |session_key: String, pinned: bool| {
        let dash = dashboard;
        let reload = reload_for_pin.clone();
        leptos::task::spawn_local(async move {
            match crate::api::sessions::set_pinned(&dash, &session_key, pinned).await {
                Ok(()) => {
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to set pinned: {e}").into());
                }
            }
        });
    });

    let reload_for_delete = reload_data;
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
                        if let Some(ws) = workspace {
                            ws.reset();
                        }
                    }
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to delete session: {e}").into());
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

    // Reusable single-row renderer (normal tile + edit-mode + delete-confirm +
    // ⋯ menu incl. pin/unpin). Wrapped in `Rc` so the three sections
    // (Pinned / Recent / Projects) can each clone and reuse it (DRY).
    // Action-state signals are read INSIDE so every row stays reactive.
    let on_select = on_select_session;
    let render_session_fn: Rc<dyn Fn(SessionEntry) -> AnyView> = Rc::new(move |session: SessionEntry| {
        let key = session.key.clone();
        let session_agent_id = session.agent_id.clone();
        let is_active = {
            let key = key.clone();
            move || chat.session_key.get().as_deref() == Some(&key)
        };
        let label = session
            .topic
            .clone()
            .unwrap_or_else(|| t_string!(i18n, chat.new_chat).to_string());
        let subtitle = format_session_subtitle(&session);
        let do_rename = do_rename.clone();
        let do_delete = do_delete.clone();
        let do_pin = do_pin.clone();

        // Determine which mode this session row is in (read signals here so
        // each row re-renders reactively on edit/delete/menu changes).
        let is_editing = editing_key.get().as_deref() == Some(&key);
        let is_deleting = deleting_key.get().as_deref() == Some(&key);
        let is_menu_open = menu_open_key.get().as_deref() == Some(&key);

        if is_editing {
            // --- Edit mode ---
            let key_for_save = key.clone();
            let key_for_save2 = key;
            let do_rename_keydown = do_rename.clone();
            let do_rename_blur = do_rename;
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
            let key_for_del = key;
            view! {
                <div
                    tabindex=0
                    class="w-full px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30
                            flex items-center justify-between text-xs outline-none"
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Escape" {
                            editing_key.set(None);
                            deleting_key.set(None);
                            menu_open_key.set(None);
                            edit_text.set(String::new());
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
                                editing_key.set(None);
                                deleting_key.set(None);
                                menu_open_key.set(None);
                                edit_text.set(String::new());
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
            let key_for_pin = key.clone();
            let pinned_now = session.pinned;
            let label_for_edit = label.clone();
            let key_for_run = key;
            let is_running = move || running.with(|m| m.contains_key(&key_for_run));
            view! {
                <div class="relative group">
                    <button
                        class=move || format!(
                            "w-full text-left px-3 py-2.5 rounded-lg text-sm flex items-center justify-between {}",
                            if is_active() {
                                "nav-tile-active"
                            } else {
                                "nav-tile"
                            }
                        )
                        on:click=move |_| {
                            editing_key.set(None);
                            deleting_key.set(None);
                            menu_open_key.set(None);
                            edit_text.set(String::new());
                            on_select(
                                key_for_click.clone(),
                                session_agent_id.clone(),
                            );
                        }
                    >
                        <div class="flex-1 min-w-0">
                            <div class="flex items-center gap-1.5">
                                <Show when=is_running>
                                    <span class="w-1.5 h-1.5 rounded-full bg-primary animate-pulse flex-shrink-0" />
                                </Show>
                                <div class="truncate font-medium text-xs">
                                    {label.clone()}
                                </div>
                            </div>
                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                {subtitle.clone()}
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
                                    editing_key.set(None);
                                    deleting_key.set(None);
                                    edit_text.set(String::new());
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
                            <div class="glass absolute right-0 top-full mt-1 z-50 min-w-[120px]
                                        bg-surface-overlay/85 border border-border rounded-lg shadow-xl
                                        py-1 text-xs">
                                <button
                                    class="w-full text-left px-3 py-1.5 text-text-secondary
                                           hover:bg-surface-sunken hover:text-text-primary transition-colors"
                                    on:click=move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        menu_open_key.set(None);
                                        do_pin(key_for_pin.clone(), !pinned_now);
                                    }
                                >
                                    {move || if pinned_now {
                                        t_string!(i18n, chat.unpin).to_string()
                                    } else {
                                        t_string!(i18n, chat.pin).to_string()
                                    }}
                                </button>
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
    });
    // Wrap in a `LocalStorage`-backed `StoredValue`: the handle is `Copy +
    // Send + Sync` (it only stores an arena index), so the section closures
    // below — which `<Show>` requires to be `Send + Sync` — can capture it,
    // even though the underlying `Rc<dyn Fn>` is neither.
    let render_session = StoredValue::new_local(render_session_fn);

    view! {
        <div class="flex flex-col h-full">
            // New Chat button (the agent switcher now lives in SidebarFooter).
            <div class="p-3 space-y-2">
                <div class="flex items-center gap-2">
                    <button
                        class="flex-1 px-3 py-1.5 rounded-lg bg-primary text-white text-sm font-medium
                               hover:bg-primary/90 transition-colors whitespace-nowrap"
                        on:click=on_new_chat
                    >
                        {move || t_string!(i18n, chat.new).to_string()}
                    </button>
                </div>

                // Search — client-side filter over the session list.
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/30">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <input
                        type="text"
                        class="flex-1 min-w-0 bg-transparent outline-none text-text-primary placeholder:text-text-tertiary"
                        placeholder=move || t_string!(i18n, chat.search_placeholder).to_string()
                        prop:value=move || search_query.get()
                        on:input=move |ev| search_query.set(event_target_value(&ev))
                    />
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
                    // Filter by the active agent — the canonical selection now
                    // lives in `chat.agent_id` (driven by SidebarFooter's
                    // switcher / SessionMap.activate).
                    let sel_agent = chat.agent_id.get();
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

                    // Filter by selected agent AND the search query, sorted by
                    // updated_at desc. Empty query → behaves exactly as before.
                    let needle = search_query.get().trim().to_lowercase();
                    let mut filtered: Vec<SessionEntry> = session_list
                        .into_iter()
                        .filter(|s| sel_agent.as_deref() == Some(&s.agent_id))
                        .filter(|s| {
                            if needle.is_empty() {
                                true
                            } else {
                                let hay = s
                                    .topic
                                    .as_deref()
                                    .unwrap_or(&s.key)
                                    .to_lowercase();
                                hay.contains(&needle)
                            }
                        })
                        .collect();
                    filtered.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

                    if filtered.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                {move || t_string!(i18n, chat.no_conversations).to_string()}
                            </p>
                        }.into_any();
                    }

                    // Partition: pinned rows appear ONLY in the Pinned section;
                    // the rest feed Recent (time buckets) and Projects (by root).
                    let now = (js_sys::Date::now() / 1000.0) as i64;
                    let (pinned, rest): (Vec<SessionEntry>, Vec<SessionEntry>) =
                        filtered.into_iter().partition(|s| s.pinned);

                    // Recent: rest sorted desc by updated_at, grouped by time bucket.
                    let mut recent = rest.clone();
                    recent.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

                    // Projects: rest grouped by project_root's last path component.
                    let ungrouped_label = t_string!(i18n, chat.ungrouped).to_string();
                    let mut by_project: std::collections::BTreeMap<String, Vec<SessionEntry>> =
                        std::collections::BTreeMap::new();
                    for s in &rest {
                        let label = s
                            .project_root
                            .as_deref()
                            .map(project_label)
                            .unwrap_or_else(|| ungrouped_label.clone());
                        by_project.entry(label).or_default().push(s.clone());
                    }

                    let render = render_session.clone();
                    let render2 = render_session.clone();
                    let render3 = render_session.clone();

                    let pinned_label = t_string!(i18n, chat.pinned).to_string();
                    let recent_label = t_string!(i18n, chat.recent).to_string();
                    let projects_label = t_string!(i18n, chat.projects).to_string();

                    view! {
                        <div class="space-y-1">
                            // ── Pinned ──
                            {
                                let has_pinned = !pinned.is_empty();
                                view! {
                                    <Show when=move || has_pinned>
                                        {
                                            let render = render.clone();
                                            let pinned = pinned.clone();
                                            let pinned_label = pinned_label.clone();
                                            view! {
                                                <SectionHeader label=pinned_label open=show_pinned />
                                                <Show when=move || show_pinned.get()>
                                                    <div class="space-y-0.5">
                                                        {
                                                            let render = render.clone();
                                                            pinned.clone().into_iter().map(move |s| render.with_value(move |f| (**f)(s))).collect_view()
                                                        }
                                                    </div>
                                                </Show>
                                            }
                                        }
                                    </Show>
                                }
                            }

                            // ── Recent (time buckets) ──
                            <SectionHeader label=recent_label open=show_recent />
                            <Show when=move || show_recent.get()>
                                {
                                    let render2 = render2.clone();
                                    let recent = recent.clone();
                                    view! {
                                        <div class="space-y-0.5">
                                            {
                                                [TimeBucket::Today, TimeBucket::ThisWeek, TimeBucket::Earlier]
                                                    .into_iter()
                                                    .map(|bucket| {
                                                        let bucket_label = match bucket {
                                                            TimeBucket::Today => t_string!(i18n, chat.today),
                                                            TimeBucket::ThisWeek => t_string!(i18n, chat.this_week),
                                                            TimeBucket::Earlier => t_string!(i18n, chat.earlier),
                                                        }.to_string();
                                                        let rows: Vec<SessionEntry> = recent
                                                            .iter()
                                                            .filter(|s| time_bucket(s.updated_at, now) == bucket)
                                                            .cloned()
                                                            .collect();
                                                        let r = render2.clone();
                                                        let has_rows = !rows.is_empty();
                                                        view! {
                                                            <Show when=move || has_rows>
                                                                {
                                                                    let r = r.clone();
                                                                    let rows = rows.clone();
                                                                    let bucket_label = bucket_label.clone();
                                                                    view! {
                                                                        <div class="px-3 pt-1 text-[10px] uppercase tracking-wider text-text-tertiary">{bucket_label}</div>
                                                                        {rows.into_iter().map(move |s| r.with_value(move |f| (**f)(s))).collect_view()}
                                                                    }
                                                                }
                                                            </Show>
                                                        }
                                                    })
                                                    .collect_view()
                                            }
                                        </div>
                                    }
                                }
                            </Show>

                            // ── Projects ──
                            <SectionHeader label=projects_label open=show_projects />
                            <Show when=move || show_projects.get()>
                                {
                                    let render3 = render3.clone();
                                    let by_project = by_project.clone();
                                    view! {
                                        <div class="space-y-0.5">
                                            {
                                                by_project.clone().into_iter().map(|(label, rows)| {
                                                    let r = render3.clone();
                                                    view! {
                                                        <div class="px-3 pt-1 text-[10px] uppercase tracking-wider text-text-tertiary">{label}</div>
                                                        {rows.into_iter().map(move |s| r.with_value(move |f| (**f)(s))).collect_view()}
                                                    }
                                                }).collect_view()
                                            }
                                        </div>
                                    }
                                }
                            </Show>
                        </div>
                    }
                    .into_any()
                }}
            </div>

            // Bottom status bar — gateway state + active run count.
            <crate::components::sidebar::SessionStatusBar />
        </div>
    }
}

/// Collapsible section header with a ▾/▸ toggle bound to an `RwSignal<bool>`.
#[component]
fn SectionHeader(label: String, open: RwSignal<bool>) -> impl IntoView {
    view! {
        <button
            class="w-full flex items-center justify-between px-3 py-1 text-[10px] font-medium
                   text-text-tertiary uppercase tracking-wider hover:text-text-secondary transition-colors"
            on:click=move |_| open.update(|v| *v = !*v)
        >
            <span>{label}</span>
            <span>{move || if open.get() { "▾" } else { "▸" }}</span>
        </button>
    }
}

/// Time bucket for the "Recent" section, derived from updated_at (epoch secs).
#[derive(Clone, Copy, PartialEq)]
enum TimeBucket {
    Today,
    ThisWeek,
    Earlier,
}

/// Classify a session's updated_at into a coarse time bucket.
/// `now` is epoch secs (from js_sys::Date::now()/1000).
fn time_bucket(updated_at: Option<i64>, now: i64) -> TimeBucket {
    let ts = updated_at.unwrap_or(0);
    let age = now.saturating_sub(ts);
    if age < 86_400 {
        TimeBucket::Today
    } else if age < 604_800 {
        TimeBucket::ThisWeek
    } else {
        TimeBucket::Earlier
    }
}

/// Last path component of a project_root, for a compact project label.
fn project_label(root: &str) -> String {
    root.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(root)
        .to_string()
}

fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    match session.updated_at {
        Some(ts) => {
            // Format Unix epoch seconds as MM-DD using js_sys::Date (WASM-safe)
            let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
            let month = date.get_month() + 1; // 0-based in JS
            let day = date.get_date();
            format!("{msg_count} msgs - {month:02}-{day:02}")
        }
        None => format!("{msg_count} messages"),
    }
}
