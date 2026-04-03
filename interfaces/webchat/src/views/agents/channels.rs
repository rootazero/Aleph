// Channels Tab — read-only binding display for an agent

use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let agent_id = StoredValue::new(agent_id);
    let bound_channel = RwSignal::new(Option::<String>::None);
    let is_loading = RwSignal::new(true);

    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash
                .rpc_call("agents.bindings", serde_json::Value::Null)
                .await
            {
                if let Some(bindings) = result.get("bindings") {
                    if let Some(ch) = bindings.get(&id).and_then(|v| v.as_str()) {
                        bound_channel.set(Some(ch.to_string()));
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
                        <div class="text-text-secondary py-8 text-center">{t!(i18n, common.loading)}</div>
                    }.into_any();
                }

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.channels.title)}</h2>
                        {move || {
                            match bound_channel.get() {
                                Some(ch) => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success">{t!(i18n, agents.channels.bound)}</span>
                                        <span class="text-sm text-text-primary">{ch}</span>
                                    </div>
                                }.into_any(),
                                None => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary">{t!(i18n, agents.channels.not_bound)}</span>
                                        <span class="text-sm text-text-secondary">{t!(i18n, agents.channels.not_bound_hint)}</span>
                                    </div>
                                }.into_any(),
                            }
                        }}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
