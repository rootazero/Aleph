// Tools Tab — Phase 1: read-only display of current tool/skill configuration

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::agents::AgentsApi;
use crate::context::DashboardState;

#[component]
pub fn ToolsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let skills_list = RwSignal::new(Vec::<String>::new());
    let subagent_policy = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);

    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = id_for_load.clone();
        spawn_local(async move {
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                let def = &detail.definition;
                if let Some(skills) = def.get("skills").and_then(|v| v.as_array()) {
                    let ids: Vec<String> = skills.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                    skills_list.set(ids);
                }
                if let Some(policy) = def.get("subagents") {
                    subagent_policy.set(serde_json::to_string_pretty(policy).unwrap_or_default());
                }
            }
            is_loading.set(false);
        });
    });

    view! {
        <div class="space-y-6">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">"Loading..."</div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-6">
                        // Skills display
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Assigned Skills"</h2>
                            {move || {
                                let skills = skills_list.get();
                                if skills.is_empty() {
                                    view! { <p class="text-sm text-text-tertiary">"No skills assigned"</p> }.into_any()
                                } else {
                                    view! {
                                        <div class="flex flex-wrap gap-2">
                                            {skills.into_iter().map(|s| view! {
                                                <span class="px-3 py-1 bg-primary/10 text-primary text-sm rounded-full">{s}</span>
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </div>

                        // Subagent policy display
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Subagent Policy"</h2>
                            {move || {
                                let policy = subagent_policy.get();
                                if policy.is_empty() {
                                    view! { <p class="text-sm text-text-tertiary">"No subagent policy configured"</p> }.into_any()
                                } else {
                                    view! {
                                        <pre class="p-4 bg-surface-sunken rounded-lg text-sm text-text-primary font-mono overflow-x-auto">{policy}</pre>
                                    }.into_any()
                                }
                            }}
                        </div>

                        // Phase 2 note
                        <div class="p-4 bg-info-subtle border border-info/20 rounded-lg text-info text-sm">
                            "Full tool management coming in Phase 2 — currently showing read-only view of configured skills and subagent policy."
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
