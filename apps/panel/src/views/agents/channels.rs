// Channels Tab — routing rule bindings and default agent selector

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use crate::api::agents::AgentsApi;
use crate::context::DashboardState;

#[derive(Debug, Clone, Deserialize)]
struct RoutingRule {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    peer_id: String,
    #[serde(default)]
    agent_id: String,
}

#[component]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);

    let rules = RwSignal::new(Vec::<RoutingRule>::new());
    let is_loading = RwSignal::new(true);
    let is_default = RwSignal::new(false);

    // Load routing rules for this agent
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash.rpc_call("routing_rules.list", serde_json::Value::Null).await {
                if let Some(arr) = result.get("rules") {
                    if let Ok(all_rules) = serde_json::from_value::<Vec<RoutingRule>>(arr.clone()) {
                        let agent_rules: Vec<RoutingRule> = all_rules
                            .into_iter()
                            .filter(|r| r.agent_id == id)
                            .collect();
                        rules.set(agent_rules);
                    }
                }
            }
            if let Ok(resp) = AgentsApi::list(&dash).await {
                is_default.set(resp.default_id == id);
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
                        // Default agent status
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Default Agent"</h2>
                            {move || {
                                if is_default.get() {
                                    view! {
                                        <div class="flex items-center gap-2 text-sm text-success">
                                            <span>"★"</span>
                                            <span>"This is the default agent for unrouted messages"</span>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex items-center justify-between">
                                            <p class="text-sm text-text-secondary">"This agent is not the default"</p>
                                            <button
                                                on:click=move |_| {
                                                    let id = agent_id.get_value();
                                                    let dash = state;
                                                    spawn_local(async move {
                                                        if AgentsApi::set_default(&dash, &id).await.is_ok() {
                                                            is_default.set(true);
                                                        }
                                                    });
                                                }
                                                class="px-4 py-1.5 bg-primary text-white rounded-lg hover:bg-primary-hover text-sm"
                                            >
                                                "Set as Default"
                                            </button>
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </div>

                        // Channel bindings
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Channel Bindings"</h2>
                            {move || {
                                let r = rules.get();
                                if r.is_empty() {
                                    view! {
                                        <p class="text-sm text-text-tertiary">"No channel bindings configured for this agent"</p>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="divide-y divide-border">
                                            {r.into_iter().map(|rule| {
                                                view! {
                                                    <div class="py-3 flex items-center justify-between">
                                                        <div>
                                                            <span class="text-sm font-medium text-text-primary">{rule.channel.clone()}</span>
                                                            {(!rule.peer_id.is_empty()).then(|| view! {
                                                                <span class="text-xs text-text-tertiary ml-2">{format!("({})", rule.peer_id)}</span>
                                                            })}
                                                        </div>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </div>

                        <div class="p-4 bg-info-subtle border border-info/20 rounded-lg text-info text-sm">
                            "Channel binding management is available in Settings → Routing Rules. This tab shows bindings assigned to this agent."
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
