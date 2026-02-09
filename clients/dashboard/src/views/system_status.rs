//! System Status view
//!
//! Displays connection status, reconnection attempts, and server health.
//! Simulates realistic connection state transitions.

use leptos::*;
use crate::context::DashboardState;
use crate::components::connection_status::ConnectionStatus;

/// Connection state for simulation
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

#[component]
pub fn SystemStatus() -> impl IntoView {
    // Get the global dashboard state
    let state = use_context::<DashboardState>()
        .expect("DashboardState must be provided");

    // Local connection state for simulation
    let (conn_state, set_conn_state) = create_signal(ConnectionState::Disconnected);
    let (error_message, set_error_message) = create_signal(None::<String>);

    // Simulate connection process
    let simulate_connection = move || {
        // Reset error
        set_error_message.set(None);
        state.reconnect_count.set(0);

        // Step 1: Connecting
        set_conn_state.set(ConnectionState::Connecting);
        state.is_connected.set(false);

        // Simulate connection delay (1.5s)
        set_timeout(
            move || {
                // 80% success rate
                let success = js_sys::Math::random() > 0.2;

                if success {
                    // Step 2: Connected
                    set_conn_state.set(ConnectionState::Connected);
                    state.is_connected.set(true);
                    log::info!("Connection successful");
                } else {
                    // Step 2: Failed
                    set_conn_state.set(ConnectionState::Failed);
                    set_error_message.set(Some("Connection failed: timeout".to_string()));
                    state.is_connected.set(false);
                    log::info!("Connection failed");
                }
            },
            std::time::Duration::from_millis(1500),
        );
    };

    // Handle connect button click
    let on_connect = move |_| {
        log::info!("Connect button clicked");
        simulate_connection();
    };

    // Handle disconnect button click
    let on_disconnect = move |_| {
        log::info!("Disconnect button clicked");
        set_conn_state.set(ConnectionState::Disconnected);
        state.is_connected.set(false);
        set_error_message.set(None);
    };

    // Get status text and color
    let status_text = move || match conn_state.get() {
        ConnectionState::Disconnected => "Disconnected",
        ConnectionState::Connecting => "Connecting...",
        ConnectionState::Connected => "Connected",
        ConnectionState::Reconnecting => "Reconnecting...",
        ConnectionState::Failed => "Failed",
    };

    let status_color = move || match conn_state.get() {
        ConnectionState::Disconnected => "text-gray-400",
        ConnectionState::Connecting => "text-amber-400",
        ConnectionState::Connected => "text-green-400",
        ConnectionState::Reconnecting => "text-blue-400",
        ConnectionState::Failed => "text-red-400",
    };

    view! {
        <div class="space-y-6">
            <div class="card">
                <h2 class="card-header">"System Status"</h2>

                // Connection Status Component
                <ConnectionStatus />

                // Detailed Status
                <div class="mt-4 bg-gray-700 rounded-lg p-4">
                    <div class="flex items-center justify-between">
                        <div>
                            <div class="text-sm text-gray-400">"Current State"</div>
                            <div class=move || format!("text-lg font-semibold {}", status_color())>
                                {status_text}
                            </div>
                        </div>
                    </div>
                </div>

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
                            class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            on:click=on_connect
                            disabled=move || conn_state.get() != ConnectionState::Disconnected && conn_state.get() != ConnectionState::Failed
                        >
                            {move || {
                                match conn_state.get() {
                                    ConnectionState::Connecting => "Connecting...",
                                    ConnectionState::Reconnecting => "Reconnecting...",
                                    _ => "Connect"
                                }
                            }}
                        </button>

                        <button
                            class="px-4 py-2 bg-red-600 hover:bg-red-700 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            on:click=on_disconnect
                            disabled=move || conn_state.get() == ConnectionState::Disconnected
                        >
                            "Disconnect"
                        </button>
                    </div>

                    // Error display
                    {move || {
                        if let Some(error) = error_message.get() {
                            view! {
                                <div class="bg-red-900/20 border border-red-500 rounded p-3 text-sm text-red-300">
                                    <strong>"Error: "</strong> {error}
                                </div>
                            }.into_view()
                        } else {
                            view! { <div></div> }.into_view()
                        }
                    }}

                    <div class="bg-blue-900/20 border border-blue-500 rounded p-3 text-sm text-blue-300">
                        <strong>"Simulation: "</strong> "Connection states transition with realistic delays. 80% success rate for initial connection."
                    </div>
                </div>
            </div>

            // Server Health (placeholder for future implementation)
            <div class="card">
                <h2 class="card-header">"Server Health"</h2>
                <div class="grid grid-cols-3 gap-4">
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Uptime"</div>
                        <div class="text-2xl font-bold text-white">"24h 15m"</div>
                    </div>
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Memory Usage"</div>
                        <div class="text-2xl font-bold text-green-400">"45%"</div>
                    </div>
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Active Sessions"</div>
                        <div class="text-2xl font-bold text-blue-400">"3"</div>
                    </div>
                </div>
            </div>
        </div>
    }
}
