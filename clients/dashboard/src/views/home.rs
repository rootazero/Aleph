//! Home view

use leptos::*;

#[component]
pub fn Home() -> impl IntoView {
    view! {
        <div class="space-y-6">
            <div class="card">
                <h2 class="card-header">"Welcome to Aleph Dashboard"</h2>
                <p class="text-gray-300 mb-4">
                    "Command Center for your AI Assistant"
                </p>
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                    <div class="bg-gray-700 rounded-lg p-4">
                        <h3 class="font-semibold mb-2">"System Status"</h3>
                        <p class="text-sm text-gray-400">
                            "Monitor connection, reconnection attempts, and server health"
                        </p>
                    </div>
                    <div class="bg-gray-700 rounded-lg p-4">
                        <h3 class="font-semibold mb-2">"Memory Explorer"</h3>
                        <p class="text-sm text-gray-400">
                            "Search and manage your AI's knowledge base"
                        </p>
                    </div>
                    <div class="bg-gray-700 rounded-lg p-4">
                        <h3 class="font-semibold mb-2">"Agent Trace"</h3>
                        <p class="text-sm text-gray-400">
                            "Real-time visualization of Agent thinking process"
                        </p>
                    </div>
                </div>
            </div>

            <div class="card">
                <h2 class="card-header">"Quick Start"</h2>
                <ol class="list-decimal list-inside space-y-2 text-gray-300">
                    <li>"Ensure Aleph Gateway is running on " <code class="bg-gray-700 px-2 py-1 rounded">"ws://127.0.0.1:18789"</code></li>
                    <li>"Navigate to " <strong>"System Status"</strong> " to connect"</li>
                    <li>"Explore the dashboard features"</li>
                </ol>
            </div>
        </div>
    }
}
