// Channels Tab — read-only binding display for an agent

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;

#[component]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);
    let bound_channel = RwSignal::new(Option::<String>::None);
    let is_loading = RwSignal::new(true);

    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash.rpc_call("agents.bindings", serde_json::Value::Null).await {
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
                        <div class="text-text-secondary py-8 text-center">"Loading..."</div>
                    }.into_any();
                }

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-4">"Channel Binding"</h2>
                        {move || {
                            match bound_channel.get() {
                                Some(ch) => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success">"BOUND"</span>
                                        <span class="text-sm text-text-primary">{ch}</span>
                                    </div>
                                }.into_any(),
                                None => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary">"未绑定"</span>
                                        <span class="text-sm text-text-secondary">"请在 Settings → Channels 中绑定此 Agent"</span>
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
