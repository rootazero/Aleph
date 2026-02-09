//! System Status view
//!
//! Displays connection status, reconnection attempts, and server health.

use leptos::*;
use crate::context::DashboardState;
use crate::components::connection_status::ConnectionStatus;

#[component]
pub fn SystemStatus() -> impl IntoView {
    // Get the global dashboard state
    let state = use_context::<DashboardState>()
        .expect("DashboardState must be provided");

    // Handle connect button click (placeholder)
    let on_connect = move |_| {
        log::info!("Connect button clicked");
        state.is_connected.set(true);
    };

    // Handle disconnect button click (placeholder)
    let on_disconnect = move |_| {
        log::info!("Disconnect button clicked");
        state.is_connected.set(false);
    };

    view! {
        <div class="space-y-6">
            <div class="card">
                <h2 class="card-header">"System Status"</h2>

                // Connection Status Component
                <ConnectionStatus />

                // Connection Controls
                <div class="mt-6 space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-300 mb-2">
                            "Gateway URL"
                        </label>
                        <input
                            type="text"
                            class="w-full bg-gray-700 border border-gray-600 rounded-md px-4 py-2 text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
                            prop:value=move || state.gateway_url.get()
                            on:input=move |ev| {
                                state.gateway_url.set(event_target_value(&ev));
                            }
                            placeholder="ws://127.0.0.1:18789"
                        />
                    </div>

                    <div class="flex space-x-4">
                        <button
                            class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed"
                            on:click=on_connect
                            disabled=move || state.is_connected.get()
                        >
                            "Connect"
                        </button>

                        <button
                            class="px-4 py-2 bg-red-600 hover:bg-red-700 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed"
                            on:click=on_disconnect
                            disabled=move || !state.is_connected.get()
                        >
                            "Disconnect"
                        </button>
                    </div>

                    <div class="bg-blue-900/20 border border-blue-500 rounded p-3 text-sm text-blue-300">
                        <strong>"Note: "</strong> "This is a POC version. WebSocket connection will be implemented in the next phase using shared_ui_logic SDK."
                    </div>
                </div>
            </div>

            // Server Health (placeholder for future implementation)
            <div class="card">
                <h2 class="card-header">"Server Health"</h2>
                <div class="text-gray-400 text-sm">
                    "Coming soon: Real-time server metrics and health indicators"
                </div>
            </div>
        </div>
    }
}
