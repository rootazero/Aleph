pub mod providers;
pub mod routing_rules;
pub mod mcp;
pub mod memory;
pub mod security;
pub mod generation_providers;
pub mod embedding_providers;
pub mod agent;
pub mod general;
pub mod shortcuts;
pub mod behavior;
pub mod search;
pub mod plugins;
pub mod skills;
pub mod policies;
pub mod channels;

pub use providers::ProvidersView;
pub use routing_rules::RoutingRulesView;
pub use mcp::McpView;
pub use memory::MemoryView;
pub use security::SecurityView;
pub use generation_providers::GenerationProvidersView;
pub use embedding_providers::EmbeddingProvidersView;
pub use agent::AgentView;
pub use general::GeneralView;
pub use shortcuts::ShortcutsView;
pub use behavior::BehaviorView;
pub use search::SearchView;
pub use plugins::PluginsView;
pub use skills::SkillsView;
pub use policies::PoliciesView;
pub use channels::TelegramChannelView;
pub use channels::DiscordChannelView;
pub use channels::WhatsAppChannelView;
pub use channels::IMessageChannelView;
pub use channels::ChannelsOverview;
pub use channels::ChannelConfigTemplate;

// Settings default view (sidebar is provided by SettingsLayout)
use leptos::prelude::*;

#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <div class="p-8 max-w-5xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    "Welcome to Settings"
                </h1>
                <p class="text-text-secondary">
                    "Select a category from the sidebar to configure Aleph Gateway"
                </p>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        "Quick Start"
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        "Configure the essential settings to get started with Aleph"
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• Set up AI providers and API keys"</li>
                        <li>"• Configure keyboard shortcuts"</li>
                        <li>"• Customize agent behavior"</li>
                        <li>"• Enable memory and knowledge base"</li>
                    </ul>
                </div>

                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        "Need Help?"
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        "Learn more about Aleph's features and configuration options"
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• Check the documentation"</li>
                        <li>"• Join the community"</li>
                        <li>"• Report issues on GitHub"</li>
                        <li>"• Contact support"</li>
                    </ul>
                </div>
            </div>
        </div>
    }
}
