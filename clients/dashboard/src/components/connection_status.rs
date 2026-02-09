//! Connection Status component
//!
//! Displays real-time connection status with visual indicators.

use leptos::*;
use crate::context::DashboardState;

#[component]
pub fn ConnectionStatus() -> impl IntoView {
    // Get the global dashboard state
    let state = use_context::<DashboardState>()
        .expect("DashboardState must be provided");

    // Derive connection status class
    let status_class = move || {
        if state.is_connected.get() {
            "status-indicator connected"
        } else {
            "status-indicator disconnected"
        }
    };

    // Derive status text
    let status_text = move || {
        if state.is_connected.get() {
            "Connected"
        } else {
            "Disconnected"
        }
    };

    // Derive status color
    let status_color = move || {
        if state.is_connected.get() {
            "text-green-400"
        } else {
            "text-red-400"
        }
    };

    view! {
        <div class="bg-gray-700 rounded-lg p-4">
            <div class="flex items-center justify-between">
                <div class="flex items-center space-x-3">
                    <span class=status_class></span>
                    <div>
                        <div class="text-sm text-gray-400">"Connection Status"</div>
                        <div class=move || format!("text-lg font-semibold {}", status_color())>
                            {status_text}
                        </div>
                    </div>
                </div>

                <div class="text-right">
                    <div class="text-sm text-gray-400">"Reconnect Attempts"</div>
                    <div class="text-lg font-semibold text-blue-400">
                        {move || state.reconnect_count.get()}
                    </div>
                </div>
            </div>

            // Connection details
            {move || {
                if state.is_connected.get() {
                    view! {
                        <div class="mt-4 pt-4 border-t border-gray-600">
                            <div class="grid grid-cols-2 gap-4 text-sm">
                                <div>
                                    <span class="text-gray-400">"Protocol: "</span>
                                    <span class="text-white">"WebSocket"</span>
                                </div>
                                <div>
                                    <span class="text-gray-400">"Transport: "</span>
                                    <span class="text-white">"WASM"</span>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! { <div></div> }.into_view()
                }
            }}
        </div>
    }
}
