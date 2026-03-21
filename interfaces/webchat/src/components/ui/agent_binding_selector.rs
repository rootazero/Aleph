// Agent Binding Selector — dropdown to bind/unbind an agent for a channel

use leptos::prelude::*;
use leptos::task::spawn_local;
use gloo_timers::future::TimeoutFuture;
use std::collections::HashMap;

use crate::api::agents::AgentsApi;
use crate::api::WorkspaceApi;
use crate::context::DashboardState;

#[component]
pub fn AgentBindingSelector(
    /// The channel ID this selector manages (e.g., "discord-default", "telegram-main")
    channel_id: String,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let channel_id = StoredValue::new(channel_id);

    // Agent list: (id, display_name)
    let agents = RwSignal::new(Vec::<(String, String)>::new());
    // All bindings: agent_id -> channel_id
    let bindings = RwSignal::new(HashMap::<String, String>::new());
    // Currently selected agent for THIS channel
    let selected = RwSignal::new(String::new()); // empty = unbound
    let is_loading = RwSignal::new(true);
    let status_msg = RwSignal::new(Option::<(bool, String)>::None); // (is_success, message)

    // Load agents and bindings when connected
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let ch_id = channel_id.get_value();
        spawn_local(async move {
            // Fetch agent list via typed API
            if let Ok(resp) = AgentsApi::list(&dash).await {
                let list: Vec<(String, String)> = resp
                    .agents
                    .iter()
                    .map(|a| {
                        let display = a.name.clone().unwrap_or_else(|| a.id.clone());
                        (a.id.clone(), display)
                    })
                    .collect();
                agents.set(list);
            }

            // Fetch bindings via typed API
            if let Ok(map) = WorkspaceApi::agent_bindings(&dash).await {
                // Find which agent is bound to this channel
                let current = map
                    .iter()
                    .find(|(_, ch)| ch.as_str() == ch_id)
                    .map(|(aid, _)| aid.clone())
                    .unwrap_or_default();
                selected.set(current);
                bindings.set(map);
            }

            is_loading.set(false);
        });
    });

    // Handle selection change
    let on_change = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        let ch_id = channel_id.get_value();
        let agent_id_opt = if value.is_empty() {
            None
        } else {
            Some(value.clone())
        };

        spawn_local(async move {
            match WorkspaceApi::set_channel_agent(
                &dash,
                &ch_id,
                agent_id_opt.as_deref(),
            )
            .await
            {
                Ok(_) => {
                    selected.set(value.clone());
                    // Update local bindings cache
                    let mut b = bindings.get_untracked();
                    // Remove old binding for this channel
                    b.retain(|_, ch| ch != &ch_id);
                    // Add new binding if not unbinding
                    if !value.is_empty() {
                        b.insert(value, ch_id.clone());
                    }
                    bindings.set(b);
                    status_msg.set(Some((true, "Binding updated".to_string())));
                }
                Err(e) => {
                    status_msg.set(Some((false, e)));
                }
            }
            // Clear status after 3 seconds
            spawn_local(async move {
                TimeoutFuture::new(3000).await;
                status_msg.set(None);
            });
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-2">"Agent Binding"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Select which agent handles messages for this channel"
            </p>
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary text-sm">"Loading..."</div>
                    }
                    .into_any();
                }

                let ch_id = channel_id.get_value();
                let current_bindings = bindings.get();
                let current_selected = selected.get();

                view! {
                    <div class="space-y-3">
                        <select
                            on:change=on_change
                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm text-text-primary focus:outline-none focus:border-primary"
                        >
                            <option value="" selected=current_selected.is_empty()>
                                {"\u{2014} \u{672A}\u{7ED1}\u{5B9A} \u{2014}"}
                            </option>
                            {agents
                                .get()
                                .into_iter()
                                .map(|(id, name)| {
                                    let is_selected = id == current_selected;
                                    let bound_to = current_bindings.get(&id).cloned();
                                    let is_bound_elsewhere = bound_to
                                        .as_ref()
                                        .is_some_and(|ch| ch != &ch_id);
                                    let label = if is_bound_elsewhere {
                                        format!(
                                            "{} (bound to {})",
                                            name,
                                            bound_to.unwrap_or_default()
                                        )
                                    } else {
                                        name
                                    };
                                    view! {
                                        <option
                                            value=id
                                            selected=is_selected
                                            disabled=is_bound_elsewhere
                                        >
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                        // Status message
                        {move || {
                            status_msg.get().map(|(ok, msg)| {
                                let class = if ok {
                                    "text-sm text-success"
                                } else {
                                    "text-sm text-danger"
                                };
                                view! { <p class=class>{msg}</p> }
                            })
                        }}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}
