// apps/panel/src/views/agents/mod.rs
//
// Agent Management — 6-tab detail view with per-agent routing.

pub mod overview;
pub mod behavior;
pub mod files;
pub mod skills;
pub mod tools;
pub mod channels;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use crate::api::agents::{AgentsApi, AgentSummary};
use crate::context::DashboardState;

/// Parse agent_id and tab from a path like /agents/{id}/{tab}
fn parse_agents_path(path: &str) -> (Option<String>, String) {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["agents"] => (None, "overview".to_string()),
        ["agents", id] => (Some(id.to_string()), "overview".to_string()),
        ["agents", id, tab, ..] => (Some(id.to_string()), tab.to_string()),
        _ => (None, "overview".to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTab {
    Overview,
    Behavior,
    Files,
    Skills,
    Tools,
    Channels,
}

impl AgentTab {
    fn from_str(s: &str) -> Self {
        match s {
            "behavior" => Self::Behavior,
            "files" => Self::Files,
            "skills" => Self::Skills,
            "tools" => Self::Tools,
            "channels" => Self::Channels,
            _ => Self::Overview,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Behavior => "Behavior",
            Self::Files => "Files",
            Self::Skills => "Skills",
            Self::Tools => "Tools",
            Self::Channels => "Channels",
        }
    }

    fn slug(&self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Behavior => "behavior",
            Self::Files => "files",
            Self::Skills => "skills",
            Self::Tools => "tools",
            Self::Channels => "channels",
        }
    }
}

const ALL_TABS: [AgentTab; 6] = [
    AgentTab::Overview,
    AgentTab::Behavior,
    AgentTab::Files,
    AgentTab::Skills,
    AgentTab::Tools,
    AgentTab::Channels,
];

#[component]
pub fn AgentsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let location = use_location();
    let navigate = StoredValue::new(use_navigate());

    // Reactive agent_id and tab from URL
    let parsed = Memo::new(move |_| parse_agents_path(&location.pathname.get()));
    let agent_id = Memo::new(move |_| parsed.get().0);
    let active_tab = Memo::new(move |_| AgentTab::from_str(&parsed.get().1));

    // Agent detail loaded from API
    let agent_summary = RwSignal::new(Option::<AgentSummary>::None);
    let agents_list = RwSignal::new(Vec::<AgentSummary>::new());
    let is_loading = RwSignal::new(true);
    let delete_error = RwSignal::new(Option::<String>::None);

    // Load agents list and find current agent
    let dash = state;
    Effect::new(move || {
        let id = agent_id.get();
        if !dash.is_connected.get() { return; }
        is_loading.set(true);
        spawn_local(async move {
            match AgentsApi::list(&dash).await {
                Ok(resp) => {
                    agents_list.set(resp.agents.clone());
                    if let Some(ref target_id) = id {
                        let found = resp.agents.iter().find(|a| &a.id == target_id).cloned();
                        agent_summary.set(found);
                    } else if let Some(first) = resp.agents.first() {
                        // No agent_id in URL — redirect to first agent
                        agent_summary.set(Some(first.clone()));
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load agents: {e}").into());
                }
            }
            is_loading.set(false);
        });
    });

    // Delete handler
    let handle_delete = move |_: web_sys::MouseEvent| {
        let Some(ref id) = agent_id.get() else { return };
        let id = id.clone();
        let dash = state;
        delete_error.set(None);
        spawn_local(async move {
            match AgentsApi::delete(&dash, &id).await {
                Ok(()) => {
                    navigate.with_value(|nav| nav("/agents", Default::default()));
                }
                Err(e) => {
                    delete_error.set(Some(e));
                }
            }
        });
    };

    view! {
        <div class="p-6 max-w-6xl mx-auto">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">"Loading agent..."</div>
                        </div>
                    }.into_any();
                }

                let Some(agent) = agent_summary.get() else {
                    return view! {
                        <div class="text-center py-12">
                            <h2 class="text-xl text-text-secondary">"No agent selected"</h2>
                            <p class="text-text-tertiary mt-2">"Select an agent from the sidebar or create a new one"</p>
                        </div>
                    }.into_any();
                };

                let current_id = agent.id.clone();
                let emoji = agent.emoji.clone().unwrap_or_default();
                let display_name = agent.name.clone().unwrap_or_else(|| agent.id.clone());
                let tab = active_tab.get();

                view! {
                    <div>
                        // Header
                        <div class="flex items-center justify-between mb-6">
                            <div class="flex items-center gap-3">
                                <span class="text-3xl">{emoji}</span>
                                <h1 class="text-2xl font-bold text-text-primary">{display_name}</h1>
                                {agent.is_default.then(|| view! {
                                    <span class="px-2 py-0.5 bg-warning/10 text-warning text-xs rounded-full font-medium">"Default"</span>
                                })}
                            </div>
                            <button
                                on:click=handle_delete
                                class="px-3 py-1.5 border border-danger/30 text-danger rounded-lg hover:bg-danger/10 text-sm transition-colors"
                                disabled=move || agent.is_default
                                title=move || if agent.is_default { "Cannot delete default agent" } else { "Delete agent" }
                            >
                                "Delete"
                            </button>
                        </div>

                        // Delete error
                        {move || delete_error.get().map(|e| view! {
                            <div class="mb-4 p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                        })}

                        // Tab bar
                        <div class="flex border-b border-border mb-6">
                            {ALL_TABS.iter().map(|t| {
                                let t = *t;
                                let href = format!("/agents/{}/{}", current_id, t.slug());
                                let is_active = t == tab;
                                view! {
                                    <a
                                        href=href
                                        class=move || {
                                            if is_active {
                                                "px-4 py-2 text-sm font-medium border-b-2 border-primary text-primary -mb-px"
                                            } else {
                                                "px-4 py-2 text-sm font-medium text-text-secondary hover:text-text-primary"
                                            }
                                        }
                                    >
                                        {t.label()}
                                    </a>
                                }
                            }).collect_view()}
                        </div>

                        // Tab content
                        <div>
                            {match tab {
                                AgentTab::Overview => view! { <overview::OverviewTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Behavior => view! { <behavior::BehaviorTab /> }.into_any(),
                                AgentTab::Files => view! { <files::FilesTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Skills => view! { <skills::SkillsTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Tools => view! { <tools::ToolsTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Channels => view! { <channels::ChannelsTab agent_id=current_id.clone() /> }.into_any(),
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
