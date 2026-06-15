//
// Persistent left-column footer — agent switcher + Gateway status dot.
//
// The agent switcher was lifted out of `ChatSidebar` so it stays visible
// across every panel mode (Chat / Dashboard / Memory / …). It drives the
// SAME canonical state the chat uses to pick its agent (`ChatState.agent_id`,
// kept in sync by `SessionMap.activate`), so switching here reloads the
// chat sidebar's per-agent session list exactly as the inline dropdown did.
//
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::connection::ConnectionPhase;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// An agent entry returned by the backend (agents.list).
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct AgentEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_default: bool,
}

#[component]
#[must_use]
pub fn SidebarFooter() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let session_map = expect_context::<SessionMap>();
    // Workspace pane is global; switching agents drops its stale tool-detail.
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

    let agents = RwSignal::new(Vec::<AgentEntry>::new());

    // Load the agent list once on mount.
    {
        let dash = dashboard;
        leptos::task::spawn_local(async move {
            match dash.rpc_call("agents.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("agents") {
                        if let Ok(list) = serde_json::from_value::<Vec<AgentEntry>>(arr.clone()) {
                            agents.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list agents: {e}").into());
                }
            }
        });
    }

    // Switch agent — opens or focuses that agent's tab. Mirrors the old
    // inline `on_agent_change`: SessionMap.activate restores the tab's
    // snapshot (incl. its session_key + agent_id), so the user picks up
    // where they left off. Don't clear the session here.
    let on_agent_change = move |ev: web_sys::Event| {
        let val = event_target_value(&ev);
        if !val.is_empty() {
            session_map.activate(chat, &val);
            if let Some(ws) = workspace {
                ws.reset();
            }
        }
    };

    view! {
        <div class="border-t border-border p-2 flex items-center gap-2 flex-shrink-0">
            <select
                class="flex-1 min-w-0 px-2 py-1.5 rounded-lg bg-surface-sunken border border-border
                       text-sm text-text-primary focus:outline-none focus:ring-2
                       focus:ring-primary/30 focus:border-primary truncate"
                on:change=on_agent_change
            >
                {move || {
                    let agent_list = agents.get();
                    let sel = chat.agent_id.get();
                    agent_list
                        .into_iter()
                        .map(|agent| {
                            let id = agent.id.clone();
                            let name = agent
                                .name
                                .clone()
                                .unwrap_or_else(|| agent.id.clone());
                            // Active agent if explicitly selected, else the
                            // backend default (matches first-open behaviour).
                            let is_selected = sel.as_deref() == Some(&id)
                                || (sel.is_none() && agent.is_default);
                            view! {
                                <option value={id} selected=is_selected>
                                    {name}
                                </option>
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </select>
            <StatusDot />
        </div>
    }
}

/// Gateway connection status dot. Projects the four `DashboardState`
/// connection signals into a single [`ConnectionPhase`] (same derivation as
/// `ConnectionStatus`) so colour + tooltip stay consistent: green=online,
/// yellow(pulsing)=reconnecting/connecting, red=offline.
#[component]
fn StatusDot() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let phase = Memo::new(move |_| {
        let error = state.connection_error.get();
        ConnectionPhase::derive(
            state.is_connected.get(),
            state.is_reconnecting.get(),
            state.reconnect_count.get(),
            error.as_deref(),
            state.has_connected_once.get(),
        )
    });

    let dot_class = move || match phase.get() {
        ConnectionPhase::Connected => "inline-block w-2 h-2 rounded-full shrink-0 bg-success",
        ConnectionPhase::Reconnecting { .. } | ConnectionPhase::Connecting => {
            "inline-block w-2 h-2 rounded-full shrink-0 bg-warning animate-pulse"
        }
        ConnectionPhase::Failed { .. } | ConnectionPhase::Initial => {
            "inline-block w-2 h-2 rounded-full shrink-0 bg-danger"
        }
    };

    // `phase` (Memo) and `i18n` are both Copy, so a fresh closure is cheap —
    // define two so each attribute owns its own (avoids relying on closure
    // Copy-ness for the title + aria-label pair).
    let status_label = move || match phase.get() {
        ConnectionPhase::Connected => t_string!(i18n, chat.status_online).to_string(),
        ConnectionPhase::Reconnecting { .. } | ConnectionPhase::Connecting => {
            t_string!(i18n, chat.status_reconnecting).to_string()
        }
        ConnectionPhase::Failed { .. } | ConnectionPhase::Initial => {
            t_string!(i18n, chat.status_offline).to_string()
        }
    };
    let aria_label = move || match phase.get() {
        ConnectionPhase::Connected => t_string!(i18n, chat.status_online).to_string(),
        ConnectionPhase::Reconnecting { .. } | ConnectionPhase::Connecting => {
            t_string!(i18n, chat.status_reconnecting).to_string()
        }
        ConnectionPhase::Failed { .. } | ConnectionPhase::Initial => {
            t_string!(i18n, chat.status_offline).to_string()
        }
    };

    view! {
        <span class=dot_class title=status_label role="img" aria-label=aria_label></span>
    }
}
