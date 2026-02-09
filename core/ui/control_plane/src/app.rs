use leptos::prelude::*;
use leptos_router::components::*;
use leptos_router::path;
use crate::views::home::Home;
use crate::views::system_status::SystemStatus;
use crate::views::agent_trace::AgentTrace;
use crate::views::memory::Memory;
use crate::components::sidebar::Sidebar;
use crate::context::DashboardContext;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <DashboardContext>
            <div class="flex h-screen bg-slate-950 text-slate-50 font-sans selection:bg-indigo-500/30">
                <Router>
                    // Left Sidebar
                    <Sidebar />

                    // Main Content
                    <main class="flex-1 overflow-y-auto relative">
                        // Background Glows
                        <div class="fixed top-0 right-0 -z-10 w-[500px] h-[500px] bg-indigo-500/10 blur-[120px] rounded-full translate-x-1/2 -translate-y-1/2"></div>
                        <div class="fixed bottom-0 left-0 -z-10 w-[400px] h-[400px] bg-emerald-500/5 blur-[100px] rounded-full -translate-x-1/2 translate-y-1/2"></div>

                        <Routes fallback=|| view! { <div class="p-8">"404 - Not Found"</div> }>
                            <Route path=path!("/") view=Home />
                            <Route path=path!("/status") view=SystemStatus />
                            <Route path=path!("/trace") view=AgentTrace />
                            <Route path=path!("/memory") view=Memory />
                        </Routes>
                    </main>
                </Router>
            </div>
        </DashboardContext>
    }
}