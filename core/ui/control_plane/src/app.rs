use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::*;
use leptos_router::path;
use crate::views::home::Home;
use crate::views::system_status::SystemStatus;
use crate::views::agent_trace::AgentTrace;
use crate::views::memory::Memory;
use crate::components::Sidebar;
use crate::components::SettingsLayout;
use crate::context::{DashboardContext, DashboardState};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <DashboardContext>
            <AppContent />
        </DashboardContext>
    }
}

#[component]
fn AppContent() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Setup WebSocket connection and alert subscriptions on mount
    Effect::new(move || {
        spawn_local(async move {
            // Connect to Gateway
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Connected to Gateway".into());

                    // Setup alert subscriptions
                    if let Err(e) = state.setup_alert_subscriptions().await {
                        web_sys::console::error_1(&format!("Failed to setup alert subscriptions: {}", e).into());
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to connect to Gateway: {}", e).into());
                }
            }
        });
    });

    // Cleanup on unmount
    on_cleanup(move || {
        spawn_local(async move {
            let _ = state.disconnect().await;
        });
    });

    view! {
        <div class="flex h-screen bg-surface text-text-primary font-sans selection:bg-primary/30">
            <Router>
                // Left Sidebar
                <Sidebar />

                // Main Content
                <main class="flex-1 overflow-y-auto relative">
                    <Routes fallback=|| view! { <div class="p-8">"404 - Not Found"</div> }>
                        <Route path=path!("/") view=Home />
                        <Route path=path!("/status") view=SystemStatus />
                        <Route path=path!("/trace") view=AgentTrace />
                        <Route path=path!("/memory") view=Memory />
                        <Route path=path!("/settings/*any") view=SettingsLayout />
                    </Routes>
                </main>
            </Router>
        </div>
    }
}