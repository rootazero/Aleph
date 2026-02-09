pub mod providers;
pub mod routing_rules;
pub mod mcp;
pub mod memory;
pub mod security;
pub mod generation_providers;

pub use providers::ProvidersView;
pub use routing_rules::RoutingRulesView;
pub use mcp::McpView;
pub use memory::MemoryView;
pub use security::SecurityView;
pub use generation_providers::GenerationProvidersView;

// Re-export Settings view
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <div class="p-8 max-w-7xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 bg-gradient-to-r from-indigo-400 to-emerald-400 bg-clip-text text-transparent">
                    "Settings"
                </h1>
                <p class="text-slate-400">
                    "Configure Aleph Gateway settings"
                </p>
            </div>

            // Settings categories grid
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
                <SettingsCard
                    href="/settings/providers"
                    title="AI Providers"
                    description="Manage AI provider configurations and API keys"
                    icon=view! {
                        <path d="M12 2L2 7l10 5 10-5-10-5z" />
                        <path d="M2 17l10 5 10-5" />
                        <path d="M2 12l10 5 10-5" />
                    }
                />

                <SettingsCard
                    href="/settings/generation-providers"
                    title="Generation Providers"
                    description="Configure image, video, and audio generation providers"
                    icon=view! {
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                        <circle cx="8.5" cy="8.5" r="1.5" />
                        <polyline points="21 15 16 10 5 21" />
                    }
                />

                <SettingsCard
                    href="/settings/routing"
                    title="Routing Rules"
                    description="Configure smart AI provider selection rules"
                    icon=view! {
                        <polyline points="16 18 22 12 16 6" />
                        <polyline points="8 6 2 12 8 18" />
                    }
                />

                <SettingsCard
                    href="/settings/memory"
                    title="Memory & Knowledge"
                    description="Configure indexing and fact extraction"
                    icon=view! {
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    }
                />

                <SettingsCard
                    href="/settings/security"
                    title="Security"
                    description="Manage authentication and access control"
                    icon=view! {
                        <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                        <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                    }
                />

                <SettingsCard
                    href="/settings/extensions"
                    title="Extensions"
                    description="Manage MCP plugins and environment variables"
                    icon=view! {
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                        <line x1="9" y1="9" x2="15" y2="15" />
                        <line x1="15" y1="9" x2="9" y2="15" />
                    }
                />

                <SettingsCard
                    href="/settings/system"
                    title="System"
                    description="Gateway settings and service discovery"
                    icon=view! {
                        <circle cx="12" cy="12" r="3" />
                        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn SettingsCard(
    href: &'static str,
    title: &'static str,
    description: &'static str,
    icon: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class="block p-6 bg-slate-900/50 backdrop-blur-sm border border-slate-800 rounded-xl hover:border-indigo-500/50 hover:bg-slate-900/70 transition-all duration-200 group"
        >
            <div class="flex items-start gap-4">
                <div class="p-3 bg-indigo-500/10 rounded-lg group-hover:bg-indigo-500/20 transition-colors">
                    <svg
                        width="24"
                        height="24"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        class="text-indigo-400"
                    >
                        {icon}
                    </svg>
                </div>
                <div class="flex-1">
                    <h3 class="text-lg font-semibold text-slate-200 mb-1 group-hover:text-white transition-colors">
                        {title}
                    </h3>
                    <p class="text-sm text-slate-400">
                        {description}
                    </p>
                </div>
                <svg
                    width="20"
                    height="20"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="text-slate-600 group-hover:text-indigo-400 transition-colors"
                >
                    <polyline points="9 18 15 12 9 6" />
                </svg>
            </div>
        </A>
    }
}

