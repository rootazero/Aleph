pub mod acp_harnesses;
pub mod auth;
pub mod behavior;
pub mod browser;
pub mod browser_runtime_banner;
pub mod channels;
pub mod clawhub;
pub mod embedding_providers;
pub mod execution;
pub mod general;
pub mod generation_providers;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod policies;
pub mod providers;
pub mod reranking_providers;
pub mod routing_rules;
pub mod runtime;
pub mod search;
pub mod security;
pub mod skills;

pub use acp_harnesses::AcpHarnessesView;
pub use auth::AuthView;
pub use behavior::BehaviorView;
pub use browser::BrowserView;
pub use channels::ChannelPlatformPage;
pub use channels::ChannelsOverview;
pub use clawhub::ClawHubView;
pub use embedding_providers::EmbeddingProvidersView;
pub use execution::ExecutionView;
pub use general::GeneralView;
pub use generation_providers::GenerationProvidersView;
pub use mcp::McpView;
pub use memory::MemoryView;
pub use plugins::PluginsView;
pub use policies::PoliciesView;
pub use providers::ProvidersView;
pub use reranking_providers::RerankingProvidersView;
pub use routing_rules::RoutingRulesView;
pub use runtime::RuntimeView;
pub use search::SearchView;
pub use security::SecurityView;
pub use skills::SkillsView;

// Settings default view (sidebar is provided by SettingsLayout)
use crate::i18n::*;
use leptos::prelude::*;

#[component]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="p-8 max-w-5xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    {t!(i18n, settings.welcome)}
                </h1>
                <p class="text-text-secondary">
                    {t!(i18n, settings.select_category)}
                </p>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        {t!(i18n, settings.quick_start.title)}
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        {t!(i18n, settings.quick_start.description)}
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• " {t!(i18n, settings.quick_start.providers)}</li>
                        <li>"• " {t!(i18n, settings.quick_start.behavior)}</li>
                        <li>"• " {t!(i18n, settings.quick_start.memory)}</li>
                    </ul>
                </div>

                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        {t!(i18n, settings.help.title)}
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        {t!(i18n, settings.help.description)}
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• " {t!(i18n, settings.help.docs)}</li>
                        <li>"• " {t!(i18n, settings.help.community)}</li>
                        <li>"• " {t!(i18n, settings.help.issues)}</li>
                        <li>"• " {t!(i18n, settings.help.support)}</li>
                    </ul>
                </div>
            </div>
        </div>
    }
}
