// Agent Binding Selector — dropdown to bind/unbind an agent for a channel

use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::agents::AgentsApi;
use crate::api::WorkspaceApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
#[must_use]
pub fn AgentBindingSelector(
    /// The channel ID this selector manages (e.g., "discord-default", "telegram-main")
    channel_id: String,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let channel_id = StoredValue::new(channel_id);

    // Agent list: (id, display_name)
    let agents = RwSignal::new(Vec::<(String, String)>::new());
    // All bindings: agent_id -> bound channels (many-to-one: an agent may
    // serve several channels at once).
    let bindings = RwSignal::new(HashMap::<String, Vec<String>>::new());
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
                    .find(|(_, chs)| chs.iter().any(|ch| ch == &ch_id))
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
            match WorkspaceApi::set_channel_agent(&dash, &ch_id, agent_id_opt.as_deref()).await {
                Ok(_) => {
                    selected.set(value.clone());
                    // Update local bindings cache: this channel moves from its
                    // old agent (if any) to the newly selected one.
                    // The selector can be unmounted while `set_channel_agent` is
                    // in flight (the settings page it lives on is a route). The
                    // write already landed server-side; only the local cache
                    // refresh is skipped, and there is no cache to refresh.
                    let Some(mut b) = bindings.try_get_untracked() else {
                        return;
                    };
                    for chs in b.values_mut() {
                        chs.retain(|ch| ch != &ch_id);
                    }
                    b.retain(|_, chs| !chs.is_empty());
                    if !value.is_empty() {
                        let chs = b.entry(value).or_default();
                        chs.push(ch_id.clone());
                        chs.sort();
                    }
                    bindings.set(b);
                    status_msg.set(Some((
                        true,
                        t_string!(i18n, common.binding_updated).to_string(),
                    )));
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
            <h2 class="text-lg font-semibold text-text-primary mb-2">{t!(i18n, common.agent_binding_title)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, common.select_agent_hint)}
            </p>
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary text-sm">{t!(i18n, common.loading)}</div>
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
                                {t!(i18n, common.unbound_option)}
                            </option>
                            {agents
                                .get()
                                .into_iter()
                                .map(|(id, name)| {
                                    let is_selected = id == current_selected;
                                    // Many-to-one binding model: an agent already
                                    // serving other channels is still selectable
                                    // here (the old UI wrongly disabled it — a
                                    // relic of the lossy one-channel-per-agent
                                    // map). Annotate for context instead.
                                    let other_channels: Vec<String> = current_bindings
                                        .get(&id)
                                        .map(|chs| {
                                            chs.iter()
                                                .filter(|ch| *ch != &ch_id)
                                                .cloned()
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let label = if other_channels.is_empty() {
                                        name
                                    } else {
                                        let bound_prefix = t_string!(i18n, common.bound_to_prefix).to_string();
                                        format!(
                                            "{} ({} {})",
                                            name,
                                            bound_prefix,
                                            other_channels.join(", ")
                                        )
                                    };
                                    view! {
                                        <option value=id selected=is_selected>
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
