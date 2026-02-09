//! Aleph Dashboard
//!
//! Web-based dashboard for Aleph AI assistant management and monitoring.

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

mod components;
mod context;
mod views;
mod models;
mod mock_data;

use context::DashboardContext;
use views::{home::Home, system_status::SystemStatus, agent_trace::AgentTrace, memory_explorer::MemoryExplorer};

#[component]
pub fn App() -> impl IntoView {
    // Provide meta tags
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/aleph-dashboard.css"/>
        <Title text="Aleph Dashboard"/>
        <Meta name="description" content="Aleph AI Assistant Dashboard"/>

        <Router>
            <DashboardContext>
                <main class="min-h-screen bg-gray-900">
                    <nav class="bg-gray-800 border-b border-gray-700">
                        <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
                            <div class="flex items-center justify-between h-16">
                                <div class="flex items-center">
                                    <h1 class="text-xl font-bold text-white">
                                        "Aleph Dashboard"
                                    </h1>
                                </div>
                                <div class="flex space-x-4">
                                    <A href="/" class="text-gray-300 hover:text-white px-3 py-2 rounded-md text-sm font-medium">
                                        "Home"
                                    </A>
                                    <A href="/system" class="text-gray-300 hover:text-white px-3 py-2 rounded-md text-sm font-medium">
                                        "System Status"
                                    </A>
                                    <A href="/trace" class="text-gray-300 hover:text-white px-3 py-2 rounded-md text-sm font-medium">
                                        "Agent Trace"
                                    </A>
                                    <A href="/memory" class="text-gray-300 hover:text-white px-3 py-2 rounded-md text-sm font-medium">
                                        "Memory"
                                    </A>
                                </div>
                            </div>
                        </div>
                    </nav>

                    <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
                        <Routes>
                            <Route path="/" view=Home/>
                            <Route path="/system" view=SystemStatus/>
                            <Route path="/trace" view=AgentTrace/>
                            <Route path="/memory" view=MemoryExplorer/>
                        </Routes>
                    </div>
                </main>
            </DashboardContext>
        </Router>
    }
}

fn main() {
    // Set up console error panic hook for better error messages in browser console
    console_error_panic_hook::set_once();

    // Initialize console logger
    console_log::init_with_level(log::Level::Debug).expect("Failed to initialize logger");

    log::info!("Starting Aleph Dashboard");

    mount_to_body(|| view! { <App/> })
}
