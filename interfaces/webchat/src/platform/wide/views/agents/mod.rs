//
// Agent Management — 5-tab detail view with per-agent routing.

pub mod channels;
pub mod files;
pub mod overview;
pub mod skills;
pub mod teams;

use crate::api::agents::{AgentSummary, AgentsApi};
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};

/// Parse `agent_id` and tab from a path like /agents/{id}/{tab}
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
    Files,
    Skills,
    Channels,
    Teams,
}

impl AgentTab {
    fn from_str(s: &str) -> Self {
        match s {
            "files" => Self::Files,
            "skills" => Self::Skills,
            "channels" => Self::Channels,
            "teams" => Self::Teams,
            _ => Self::Overview,
        }
    }

    const fn slug(&self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Files => "files",
            Self::Skills => "skills",
            Self::Channels => "channels",
            Self::Teams => "teams",
        }
    }
}

const ALL_TABS: [AgentTab; 5] = [
    AgentTab::Overview,
    AgentTab::Files,
    AgentTab::Skills,
    AgentTab::Channels,
    AgentTab::Teams,
];

#[component]
#[must_use]
pub fn AgentsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let location = use_location();
    let navigate = StoredValue::new(use_navigate());
    let i18n = use_i18n();

    // Reactive agent_id and tab from URL
    let parsed = Memo::new(move |_| parse_agents_path(&location.pathname.get()));
    let agent_id = Memo::new(move |_| parsed.get().0);
    let active_tab = Memo::new(move |_| AgentTab::from_str(&parsed.get().1));

    // Agent detail loaded from API
    let agent_summary = RwSignal::new(Option::<AgentSummary>::None);
    let agents_list = RwSignal::new(Vec::<AgentSummary>::new());
    let is_loading = RwSignal::new(true);
    let delete_error = RwSignal::new(Option::<String>::None);
    let load_error = RwSignal::new(Option::<String>::None);

    // Load agents list and find current agent
    let dash = state;
    Effect::new(move || {
        let id = agent_id.get();
        if !dash.is_connected.get() {
            return;
        }
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
                    load_error.set(Some(
                        crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                }
            }
            is_loading.set(false);
        });
    });

    // Delete handler
    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
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
                    delete_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                }
            }
        });
    };

    view! {
        <div class="px-6 pb-6 aleph-content-top max-w-6xl mx-auto">
            {move || load_error.get().map(|e| view! {
                <div class="mb-4 p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">{t!(i18n, agents.loading)}</div>
                        </div>
                    }.into_any();
                }

                let Some(agent) = agent_summary.get() else {
                    return view! {
                        <div class="text-center py-12">
                            <h2 class="text-xl text-text-secondary">{t!(i18n, agents.no_agent_selected)}</h2>
                            <p class="text-text-tertiary mt-2">{t!(i18n, agents.no_agent_hint)}</p>
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
                                    <span class="px-2 py-0.5 bg-warning/10 text-warning text-xs rounded-full font-medium">{t!(i18n, agents.default_badge)}</span>
                                })}
                            </div>
                            {move || if confirming.get() {
                                view! {
                                    <ConfirmButton confirming=confirming on_confirm=on_confirm_delete size_class="px-3 py-1.5 text-sm" />
                                }.into_any()
                            } else {
                                view! {
                                    <button
                                        on:click=move |_| confirming.set(true)
                                        class="px-3 py-1.5 border border-danger/30 text-danger rounded-lg hover:bg-danger/10 text-sm transition-colors"
                                        disabled=move || agent.is_default
                                        title=move || if agent.is_default { t_string!(i18n, agents.cannot_delete_default).to_string() } else { t_string!(i18n, agents.delete_agent).to_string() }
                                    >
                                        {t!(i18n, agents.delete)}
                                    </button>
                                }.into_any()
                            }}
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
                                let label = match t {
                                    AgentTab::Overview => t_string!(i18n, agents.tabs.overview).to_string(),
                                    AgentTab::Files => t_string!(i18n, agents.tabs.files).to_string(),
                                    AgentTab::Skills => t_string!(i18n, agents.tabs.skills).to_string(),
                                    AgentTab::Channels => t_string!(i18n, agents.tabs.channels).to_string(),
                                    AgentTab::Teams => t_string!(i18n, agents.tabs.teams).to_string(),
                                };
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
                                        {label}
                                    </a>
                                }
                            }).collect_view()}
                        </div>

                        // Tab content
                        <div>
                            {match tab {
                                AgentTab::Overview => view! { <overview::OverviewTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Files => view! { <files::FilesTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Skills => view! { <skills::SkillsTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Channels => view! { <channels::ChannelsTab agent_id=current_id.clone() /> }.into_any(),
                                AgentTab::Teams => view! { <teams::TeamsTab agent_id=current_id.clone() /> }.into_any(),
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
