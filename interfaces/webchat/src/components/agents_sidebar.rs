//
// Agents mode sidebar — agent list with create, select, and default agent controls.
//
use crate::api::agent_binding::AgentBindingApi;
use crate::api::agents::{AgentSummary, AgentsApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;
use std::collections::HashMap;

#[component]
#[must_use]
pub fn AgentsSidebar() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let location = use_location();
    let i18n = use_i18n();

    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_id = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let show_create = RwSignal::new(false);
    let new_agent_id = RwSignal::new(String::new());
    let new_agent_name = RwSignal::new(String::new());
    let new_agent_archetype = RwSignal::new("assistant".to_string());
    let create_error = RwSignal::new(Option::<String>::None);

    // Filter state: "all" | "channel" | "standalone"
    let filter = RwSignal::new("all".to_string());
    // agent_id → bound channels (many-to-one: an agent may serve several)
    let bindings = RwSignal::new(HashMap::<String, Vec<String>>::new());

    // Reload agents list
    let reload = move || {
        let dash = state;
        spawn_local(async move {
            match AgentsApi::list(&dash).await {
                Ok(resp) => {
                    default_id.set(resp.default_id);
                    agents.set(resp.agents);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list agents: {e}").into());
                }
            }
            is_loading.set(false);
        });
    };

    // Load on mount when connected
    Effect::new(move || {
        if state.is_connected.get() {
            reload();
            // Load channel bindings
            let dash = state;
            spawn_local(async move {
                match AgentBindingApi::agent_bindings(&dash).await {
                    Ok(map) => bindings.set(map),
                    Err(e) => {
                        web_sys::console::warn_1(
                            &format!("Failed to load agent bindings: {e}").into(),
                        );
                    }
                }
            });
        }
    });

    view! {
        <div class="flex flex-col h-full">
            // Header + Create button
            <div class="p-3 border-b border-border space-y-2">
                <button
                    on:click=move |_| show_create.update(|v| *v = !*v)
                    class="w-full px-3 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover transition-colors text-sm font-medium"
                >
                    {t!(i18n, agents.sidebar.new_agent)}
                </button>
                // Filter dropdown
                <select
                    on:change=move |ev| filter.set(event_target_value(&ev))
                    class="w-full px-3 py-1.5 bg-surface-secondary border border-border rounded-md text-xs text-text-secondary focus:outline-none focus:ring-1 focus:ring-primary/30"
                >
                    <option value="all">{t!(i18n, agents.sidebar.filter_all)}</option>
                    <option value="channel">{t!(i18n, agents.sidebar.filter_channel)}</option>
                    <option value="standalone">{t!(i18n, agents.sidebar.filter_standalone)}</option>
                </select>
            </div>

            // Create form (collapsible)
            {move || show_create.get().then(|| view! {
                <div class="p-3 border-b border-border space-y-2">
                    <input
                        type="text"
                        placeholder=move || t_string!(i18n, agents.sidebar.agent_id_placeholder).to_string()
                        prop:value=move || new_agent_id.get()
                        on:input=move |ev| new_agent_id.set(event_target_value(&ev))
                        class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                    />
                    <input
                        type="text"
                        placeholder=move || t_string!(i18n, agents.sidebar.display_name_placeholder).to_string()
                        prop:value=move || new_agent_name.get()
                        on:input=move |ev| new_agent_name.set(event_target_value(&ev))
                        class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                    />
                    // Soul archetype (template) selector
                    <select
                        title=move || t_string!(i18n, agents.sidebar.archetype_label).to_string()
                        on:change=move |ev| new_agent_archetype.set(event_target_value(&ev))
                        prop:value=move || new_agent_archetype.get()
                        class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                    >
                        <option value="assistant">{t!(i18n, agents.sidebar.archetype_assistant)}</option>
                        <option value="expert">{t!(i18n, agents.sidebar.archetype_expert)}</option>
                        <option value="maker">{t!(i18n, agents.sidebar.archetype_maker)}</option>
                        <option value="companion">{t!(i18n, agents.sidebar.archetype_companion)}</option>
                    </select>
                    {move || create_error.get().map(|e| view! {
                        <p class="text-xs text-danger">{e}</p>
                    })}
                    <div class="flex gap-2">
                        <button
                            on:click=move |_| {
                                let id = new_agent_id.get();
                                let name_val = new_agent_name.get();
                                if id.is_empty() {
                                    create_error.set(Some(t_string!(i18n, agents.sidebar.id_required).to_string()));
                                    return;
                                }
                                create_error.set(None);
                                let name = if name_val.is_empty() { None } else { Some(name_val) };
                                let archetype = new_agent_archetype.get();
                                let dash = state;
                                spawn_local(async move {
                                    match AgentsApi::create(&dash, &id, name.as_deref(), None, Some(&archetype)).await {
                                        Ok(()) => {
                                            show_create.set(false);
                                            new_agent_id.set(String::new());
                                            new_agent_name.set(String::new());
                                            new_agent_archetype.set("assistant".to_string());
                                            reload();
                                        }
                                        Err(e) => {
                                            create_error.set(Some(e));
                                        }
                                    }
                                });
                            }
                            class="flex-1 px-2 py-1.5 bg-primary text-white rounded text-sm hover:bg-primary-hover"
                        >
                            {t!(i18n, agents.sidebar.create)}
                        </button>
                        <button
                            on:click=move |_| show_create.set(false)
                            class="px-2 py-1.5 border border-border rounded text-sm text-text-secondary hover:bg-surface-raised"
                        >
                            {t!(i18n, common.cancel)}
                        </button>
                    </div>
                </div>
            })}

            // Agent list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if is_loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-tertiary text-sm">{t!(i18n, common.loading)}</div>
                        }.into_any()
                    } else {
                        let current_path = location.pathname.get();
                        let current_filter = filter.get();
                        let current_bindings = bindings.get();

                        view! {
                            <div class="py-1">
                                {agents.get().into_iter().filter(|agent| {
                                    match current_filter.as_str() {
                                        "channel" => current_bindings.contains_key(&agent.id),
                                        "standalone" => !current_bindings.contains_key(&agent.id),
                                        _ => true, // "all"
                                    }
                                }).map(|agent| {
                                    let agent_path = format!("/agents/{}/overview", agent.id);
                                    let is_active = current_path.starts_with(&format!("/agents/{}", agent.id));
                                    let is_default = agent.is_default;
                                    let emoji = agent.emoji.clone().unwrap_or_default();
                                    let display_name = agent.name.clone().unwrap_or_else(|| agent.id.clone());
                                    // Channel badge for bound agents (shown in "all" and "channel"
                                    // views). Joined list — an agent may serve several channels.
                                    let channel_badge = current_bindings
                                        .get(&agent.id)
                                        .filter(|chs| !chs.is_empty())
                                        .map(|chs| chs.join(", "));

                                    view! {
                                        <a
                                            href=agent_path
                                            class=move || {
                                                if is_active {
                                                    "nav-tile-active flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm"
                                                } else {
                                                    "nav-tile flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm"
                                                }
                                            }
                                        >
                                            <span class="text-base">{emoji}</span>
                                            <span class="flex-1 truncate">{display_name}</span>
                                            {channel_badge.map(|ch| {
                                                let title = ch.clone();
                                                view! {
                                                    <span
                                                        class="text-xs px-1.5 py-0.5 bg-primary/10 text-primary rounded truncate max-w-16"
                                                        title=title
                                                    >{ch}</span>
                                                }
                                            })}
                                            {is_default.then(|| view! {
                                                <span class="text-xs text-warning" title="Default agent">"★"</span>
                                            })}
                                        </a>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            // Default agent selector
            <div class="p-3 border-t border-border">
                <label class="block text-xs text-text-tertiary mb-1">{t!(i18n, agents.sidebar.default_agent)}</label>
                <select
                    on:change=move |ev| {
                        let id = event_target_value(&ev);
                        if id.is_empty() { return; }
                        let dash = state;
                        spawn_local(async move {
                            if AgentsApi::set_default(&dash, &id).await.is_ok() {
                                reload();
                            }
                        });
                    }
                    class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                >
                    {move || agents.get().into_iter().map(|agent| {
                        let id = agent.id.clone();
                        let name = agent.name.clone().unwrap_or_else(|| agent.id.clone());
                        let selected = id == default_id.get();
                        view! {
                            <option value=id selected=selected>{name}</option>
                        }
                    }).collect_view()}
                </select>
            </div>
        </div>
    }
}
