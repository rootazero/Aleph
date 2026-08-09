// Channels Tab — read-only binding display for an agent

use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let agent_id = StoredValue::new(agent_id);
    // Many-to-one model: an agent may be bound to several channels — show all.
    let bound_channels = RwSignal::new(Vec::<String>::new());
    let is_loading = RwSignal::new(true);

    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(map) = crate::api::AgentBindingApi::agent_bindings(&dash).await {
                if let Some(chs) = map.get(&id) {
                    bound_channels.set(chs.clone());
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
                        <div class="text-text-secondary py-8 text-center">{t!(i18n, common.loading)}</div>
                    }.into_any();
                }

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.channels.title)}</h2>
                        {move || {
                            let chs = bound_channels.get();
                            if chs.is_empty() {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary">{t!(i18n, agents.channels.not_bound)}</span>
                                        <span class="text-sm text-text-secondary">{t!(i18n, agents.channels.not_bound_hint)}</span>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="flex items-center gap-2 flex-wrap">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success">{t!(i18n, agents.channels.bound)}</span>
                                        {chs.into_iter().map(|ch| view! {
                                            <span class="text-sm text-text-primary">{ch}</span>
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
