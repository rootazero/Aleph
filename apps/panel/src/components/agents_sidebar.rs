// apps/panel/src/components/agents_sidebar.rs
//
// Agents mode sidebar — agent list with create, select, and default agent controls.
//
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;
use crate::api::agents::{AgentSummary, AgentsApi};
use crate::context::DashboardState;

#[component]
pub fn AgentsSidebar() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let location = use_location();

    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_id = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let show_create = RwSignal::new(false);
    let new_agent_id = RwSignal::new(String::new());
    let new_agent_name = RwSignal::new(String::new());
    let create_error = RwSignal::new(Option::<String>::None);

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
        }
    });

    view! {
        <div class="flex flex-col h-full">
            // Header + Create button
            <div class="p-3 border-b border-border">
                <button
                    on:click=move |_| show_create.update(|v| *v = !*v)
                    class="w-full px-3 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover transition-colors text-sm font-medium"
                >
                    "+ New Agent"
                </button>
            </div>

            // Create form (collapsible)
            {move || show_create.get().then(|| view! {
                <div class="p-3 border-b border-border space-y-2">
                    <input
                        type="text"
                        placeholder="Agent ID (e.g. coder)"
                        prop:value=move || new_agent_id.get()
                        on:input=move |ev| new_agent_id.set(event_target_value(&ev))
                        class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                    />
                    <input
                        type="text"
                        placeholder="Display Name (optional)"
                        prop:value=move || new_agent_name.get()
                        on:input=move |ev| new_agent_name.set(event_target_value(&ev))
                        class="w-full px-2 py-1.5 bg-surface-sunken border border-border rounded text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary/30"
                    />
                    {move || create_error.get().map(|e| view! {
                        <p class="text-xs text-danger">{e}</p>
                    })}
                    <div class="flex gap-2">
                        <button
                            on:click=move |_| {
                                let id = new_agent_id.get();
                                let name_val = new_agent_name.get();
                                if id.is_empty() {
                                    create_error.set(Some("Agent ID is required".to_string()));
                                    return;
                                }
                                create_error.set(None);
                                let name = if name_val.is_empty() { None } else { Some(name_val) };
                                let dash = state;
                                spawn_local(async move {
                                    match AgentsApi::create(&dash, &id, name.as_deref(), None).await {
                                        Ok(()) => {
                                            show_create.set(false);
                                            new_agent_id.set(String::new());
                                            new_agent_name.set(String::new());
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
                            "Create"
                        </button>
                        <button
                            on:click=move |_| show_create.set(false)
                            class="px-2 py-1.5 border border-border rounded text-sm text-text-secondary hover:bg-surface-raised"
                        >
                            "Cancel"
                        </button>
                    </div>
                </div>
            })}

            // Agent list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if is_loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-tertiary text-sm">"Loading..."</div>
                        }.into_any()
                    } else {
                        let current_path = location.pathname.get();
                        view! {
                            <div class="py-1">
                                {agents.get().into_iter().map(|agent| {
                                    let agent_path = format!("/agents/{}/overview", agent.id);
                                    let is_active = current_path.starts_with(&format!("/agents/{}", agent.id));
                                    let is_default = agent.is_default;
                                    let emoji = agent.emoji.clone().unwrap_or_default();
                                    let display_name = agent.name.clone().unwrap_or_else(|| agent.id.clone());

                                    view! {
                                        <a
                                            href=agent_path
                                            class=move || {
                                                if is_active {
                                                    "flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm bg-sidebar-active text-sidebar-accent font-medium"
                                                } else {
                                                    "flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                                                }
                                            }
                                        >
                                            <span class="text-base">{emoji}</span>
                                            <span class="flex-1 truncate">{display_name}</span>
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
                <label class="block text-xs text-text-tertiary mb-1">"Default Agent"</label>
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
