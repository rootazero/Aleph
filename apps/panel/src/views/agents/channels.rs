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

#[derive(Debug, Clone, Deserialize)]
struct ChannelInfo {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    channel_type: String,
}

#[component]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);

    let rules = RwSignal::new(Vec::<RoutingRule>::new());
    let is_loading = RwSignal::new(true);
    let is_default = RwSignal::new(false);
    let all_channels = RwSignal::new(Vec::<ChannelInfo>::new());
    let allowed_links = RwSignal::new(Option::<Vec<String>>::None);
    let is_all_allowed = RwSignal::new(true);

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

            // Load all channels (links)
            if let Ok(result) = dash.rpc_call("channels.list", serde_json::Value::Null).await {
                if let Ok(channels) = serde_json::from_value::<Vec<ChannelInfo>>(result) {
                    all_channels.set(channels);
                }
            }

            // Load agent's allowed_links from definition
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                if let Some(links) = detail.definition.get("allowed_links") {
                    if let Ok(links_vec) = serde_json::from_value::<Vec<String>>(links.clone()) {
                        if links_vec.is_empty() {
                            is_all_allowed.set(true);
                            allowed_links.set(None);
                        } else {
                            is_all_allowed.set(false);
                            allowed_links.set(Some(links_vec));
                        }
                    }
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

                        // Link Access Control
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Link Access Control"</h2>
                            <p class="text-sm text-text-tertiary mb-4">"Control which bots can access this agent. All bots are allowed by default."</p>
                            {move || {
                                let channels = all_channels.get();
                                if channels.is_empty() {
                                    return view! {
                                        <p class="text-sm text-text-tertiary">"No active links found"</p>
                                    }.into_any();
                                }

                                view! {
                                    <div class="divide-y divide-border">
                                        {channels.into_iter().map(|ch| {
                                            let ch_id_for_click = ch.id.clone();
                                            let ch_id_for_class = ch.id.clone();
                                            let ch_id_for_text = ch.id.clone();
                                            let display_name = if ch.name.is_empty() { ch.id.clone() } else { ch.name.clone() };
                                            let ch_type = ch.channel_type.clone();

                                            view! {
                                                <div class="py-3 flex items-center justify-between">
                                                    <div>
                                                        <span class="text-sm font-medium text-text-primary">{display_name}</span>
                                                        <span class="text-xs text-text-tertiary ml-2">{format!("({})", ch_type)}</span>
                                                    </div>
                                                    <button
                                                        on:click=move |_| {
                                                            let ch_id = ch_id_for_click.clone();
                                                            let aid = agent_id.get_value();
                                                            let dash = state;
                                                            spawn_local(async move {
                                                                let all_chs = all_channels.get_untracked();
                                                                let all_ids: Vec<String> = all_chs.iter().map(|c| c.id.clone()).collect();

                                                                let new_list = if is_all_allowed.get_untracked() {
                                                                    // Currently all allowed, user toggled one OFF
                                                                    all_ids.into_iter().filter(|id| id != &ch_id).collect::<Vec<_>>()
                                                                } else {
                                                                    let mut list = allowed_links.get_untracked().unwrap_or_default();
                                                                    if list.contains(&ch_id) {
                                                                        list.retain(|id| id != &ch_id);
                                                                    } else {
                                                                        list.push(ch_id.clone());
                                                                    }
                                                                    list
                                                                };

                                                                let all_chs = all_channels.get_untracked();
                                                                let all_on = all_chs.iter().all(|c| new_list.contains(&c.id));

                                                                let patch = if all_on {
                                                                    serde_json::json!({"allowed_links": []})
                                                                } else {
                                                                    serde_json::json!({"allowed_links": new_list.clone()})
                                                                };

                                                                if AgentsApi::update(&dash, &aid, patch).await.is_ok() {
                                                                    if all_on {
                                                                        is_all_allowed.set(true);
                                                                        allowed_links.set(None);
                                                                    } else {
                                                                        is_all_allowed.set(false);
                                                                        allowed_links.set(Some(new_list));
                                                                    }
                                                                }
                                                            });
                                                        }
                                                        class=move || {
                                                            let on = if is_all_allowed.get() {
                                                                true
                                                            } else if let Some(ref list) = allowed_links.get() {
                                                                list.contains(&ch_id_for_class)
                                                            } else {
                                                                true
                                                            };
                                                            if on {
                                                                "px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success cursor-pointer"
                                                            } else {
                                                                "px-3 py-1 rounded-full text-xs font-medium bg-error/20 text-error cursor-pointer"
                                                            }
                                                        }
                                                    >
                                                        {move || {
                                                            let on = if is_all_allowed.get() {
                                                                true
                                                            } else if let Some(ref list) = allowed_links.get() {
                                                                list.contains(&ch_id_for_text)
                                                            } else {
                                                                true
                                                            };
                                                            if on { "ON" } else { "OFF" }
                                                        }}
                                                    </button>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
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
