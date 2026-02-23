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
            "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.5)]"
        } else {
            "bg-amber-500 shadow-[0_0_8px_rgba(245,158,11,0.5)]"
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
        <div class="bg-slate-900/40 border border-slate-800 rounded-2xl p-4 backdrop-blur-sm">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-3">
                    <div class=format!("w-2 h-2 rounded-full {}", status_class())></div>
                    <span class="text-sm font-medium">{status_text()}</span>
                </div>
                
                {move || if !is_connected.get() {
                    view! {
                        <div class="text-xs text-slate-500">
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