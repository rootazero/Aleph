use leptos::prelude::*;
use crate::context::DashboardState;

#[component]
pub fn ConnectionStatus() -> impl IntoView {
    let state = use_context::<DashboardState>()
        .expect("DashboardState not provided");

    let is_connected = state.is_connected;
    let reconnect_count = state.reconnect_count;

    let status_class = move || {
        if is_connected.get() {
            "bg-success"
        } else {
            "bg-warning"
        }
    };

    let status_text = move || {
        if is_connected.get() {
            "Connected"
        } else {
            "Disconnected"
        }
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-2xl p-4">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-3">
                    <div class=format!("w-2 h-2 rounded-full {}", status_class())></div>
                    <span class="text-sm font-medium">{status_text()}</span>
                </div>
                
                {move || if !is_connected.get() {
                    view! {
                        <div class="text-xs text-text-tertiary">
                            "Reconnecting... (" {reconnect_count.get()} ")"
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }}
            </div>
        </div>
    }
}