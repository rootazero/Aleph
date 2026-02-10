pub mod providers;
pub mod routing_rules;
pub mod mcp;
pub mod memory;
pub mod security;
pub mod generation_providers;
pub mod agent;
pub mod general;
pub mod shortcuts;
pub mod behavior;
pub mod generation;
pub mod search;

pub use providers::ProvidersView;
pub use routing_rules::RoutingRulesView;
pub use mcp::McpView;
pub use memory::MemoryView;
pub use security::SecurityView;
pub use generation_providers::GenerationProvidersView;
pub use agent::AgentView;
pub use general::GeneralView;
pub use shortcuts::ShortcutsView;
pub use behavior::BehaviorView;
pub use generation::GenerationView;
pub use search::SearchView;

// Re-export Settings view with new sidebar layout
use leptos::prelude::*;
use crate::components::SettingsSidebar;

#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <div class="flex h-screen bg-slate-950">
            <SettingsSidebar />
            <div class="flex-1 overflow-y-auto">
                <div class="p-8 max-w-5xl mx-auto">
                    <div class="mb-8">
                        <h1 class="text-3xl font-bold mb-2 bg-gradient-to-r from-indigo-400 to-emerald-400 bg-clip-text text-transparent">
                            "Welcome to Settings"
                        </h1>
                        <p class="text-slate-400">
                            "Select a category from the sidebar to configure Aleph Gateway"
                        </p>
                    </div>

                    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <div class="p-6 bg-slate-900/50 backdrop-blur-sm border border-slate-800 rounded-xl">
                            <h3 class="text-lg font-semibold text-slate-200 mb-2">
                                "Quick Start"
                            </h3>
                            <p class="text-sm text-slate-400 mb-4">
                                "Configure the essential settings to get started with Aleph"
                            </p>
                            <ul class="space-y-2 text-sm text-slate-300">
                                <li>"• Set up AI providers and API keys"</li>
                                <li>"• Configure keyboard shortcuts"</li>
                                <li>"• Customize agent behavior"</li>
                                <li>"• Enable memory and knowledge base"</li>
                            </ul>
                        </div>

                        <div class="p-6 bg-slate-900/50 backdrop-blur-sm border border-slate-800 rounded-xl">
                            <h3 class="text-lg font-semibold text-slate-200 mb-2">
                                "Need Help?"
                            </h3>
                            <p class="text-sm text-slate-400 mb-4">
                                "Learn more about Aleph's features and configuration options"
                            </p>
                            <ul class="space-y-2 text-sm text-slate-300">
                                <li>"• Check the documentation"</li>
                                <li>"• Join the community"</li>
                                <li>"• Report issues on GitHub"</li>
                                <li>"• Contact support"</li>
                            </ul>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}
