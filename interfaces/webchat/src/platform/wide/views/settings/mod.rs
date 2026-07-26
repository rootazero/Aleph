pub mod acp_harnesses;
pub mod appearance;
pub mod behavior;
pub mod browser;
pub mod browser_runtime_banner;
pub mod channels;
pub mod desktop_autostart;
pub mod embedding_providers;
pub mod execution;
pub mod general;
pub mod generation_providers;
pub mod mcp;
pub mod memory;
pub mod moa;
pub mod network;
pub mod plugins;
pub mod policies;
pub mod providers;
pub mod reranking_providers;
pub mod route;
pub mod routing_rules;
pub mod search;
pub mod security;
pub mod skills;

pub use acp_harnesses::AcpHarnessesView;
pub use appearance::AppearanceView;
pub use behavior::BehaviorView;
pub use browser::BrowserView;
pub use channels::ChannelPlatformPage;
pub use channels::ChannelsOverview;
pub use embedding_providers::EmbeddingProvidersView;
pub use execution::ExecutionView;
pub use general::GeneralView;
pub use generation_providers::GenerationProvidersView;
pub use mcp::McpView;
pub use memory::MemoryView;
pub use moa::MoaView;
pub use network::NetworkView;
pub use plugins::PluginsView;
pub use policies::PoliciesView;
pub use providers::ProvidersView;
pub use reranking_providers::RerankingProvidersView;
pub use route::RouteView;
pub use routing_rules::RoutingRulesView;
pub use search::SearchView;
pub use security::SecurityView;
pub use skills::SkillsView;

// Settings default view (sidebar is provided by SettingsLayout)
use crate::api::{MemoryApi, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos_router::components::A;

/// Single step in the Quick Setup checklist.
#[derive(Clone)]
struct SetupStep {
    title: &'static str,
    body: &'static str,
    href: &'static str,
    cta: &'static str,
}

#[component]
#[must_use]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    // Live status — None = not yet queried, Some(true) = done, Some(false) = pending.
    let providers_ready = RwSignal::new(None::<bool>);
    let generation_ready = RwSignal::new(None::<bool>);
    let memory_ready = RwSignal::new(None::<bool>);
    let moa_ready = RwSignal::new(None::<bool>);

    Effect::new(move || {
        if !state.is_connected.get() {
            providers_ready.set(None);
            generation_ready.set(None);
            memory_ready.set(None);
            moa_ready.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            // Step 1 — at least one chat provider configured.
            match ProvidersApi::list(&state).await {
                Ok(list) => providers_ready.set(Some(!list.is_empty())),
                Err(_) => providers_ready.set(Some(false)),
            }

            // Step 2 — at least one generation provider configured.
            match crate::api::generation_providers::GenerationProvidersApi::list(&state).await {
                Ok(list) => generation_ready.set(Some(!list.is_empty())),
                Err(_) => generation_ready.set(Some(false)),
            }

            // Step 3 — memory backend online (any stat row returns).
            match MemoryApi::stats(&state, "main").await {
                Ok(_) => memory_ready.set(Some(true)),
                Err(_) => memory_ready.set(Some(false)),
            }

            // Step 4 — at least one MoA preset configured (optional).
            match crate::api::moa::MoaApi::list_presets(&state).await {
                Ok(cfg) => moa_ready.set(Some(!cfg.presets.is_empty())),
                Err(_) => moa_ready.set(Some(false)),
            }
        });
    });

    let steps = [
        SetupStep {
            title: "Configure a chat provider",
            body: "Add at least one LLM provider (Anthropic, OpenAI, Gemini, …). Without one, agents cannot respond.",
            href: "/settings/providers",
            cta: "Open Providers",
        },
        SetupStep {
            title: "Configure a generation provider (optional)",
            body: "Image / video / TTS providers power generation tools. 20+ presets available.",
            href: "/settings/generation-providers",
            cta: "Open Generation",
        },
        SetupStep {
            title: "Verify memory backend",
            body: "Memory is the long-term substrate for facts, graph nodes, and dream notes.",
            href: "/settings/memory",
            cta: "Open Memory",
        },
        SetupStep {
            title: "Configure MoA presets (optional)",
            body: "Mixture-of-Agents: multiple advisor models consult before one aggregator model responds.",
            href: "/settings/moa",
            cta: "Open MoA",
        },
    ];

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10">
            <div>
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    {t!(i18n, settings.welcome)}
                </h1>
                <p class="text-text-secondary">
                    {t!(i18n, settings.select_category)}
                </p>
            </div>

            // Quick Setup status checklist — replaces the prior static
            // "quick_start" card with live state-aware rows.
            <section class="space-y-4">
                <div class="flex items-center justify-between">
                    <h2 class="text-xl font-semibold text-text-primary">
                        {t!(i18n, settings.quick_start.title)}
                    </h2>
                    {move || {
                        let done = [providers_ready.get(), generation_ready.get(), memory_ready.get(), moa_ready.get()]
                            .iter()
                            .filter(|s| **s == Some(true))
                            .count();
                        let total = 4usize;
                        view! {
                            <span class="text-xs font-mono text-text-tertiary">
                                {format!("{done}/{total} ready")}
                            </span>
                        }
                    }}
                </div>

                <div class="space-y-3">
                    {move || {
                        let statuses = [providers_ready.get(), generation_ready.get(), memory_ready.get(), moa_ready.get()];
                        steps.iter().zip(statuses.iter()).map(|(step, status)| {
                            view! { <SetupRow step=step.clone() status=*status /> }
                        }).collect_view()
                    }}
                </div>
            </section>

            // Help card — kept; reads from the same i18n keys as before.
            <section>
                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        {t!(i18n, settings.help.title)}
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        {t!(i18n, settings.help.description)}
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>
                            "• "
                            <a
                                href="https://heyaleph.com"
                                target="_blank"
                                rel="noopener"
                                class="text-primary hover:underline"
                            >
                                {t!(i18n, settings.help.homepage)}
                            </a>
                        </li>
                        <li>
                            "• "
                            <a
                                href="https://docs.heyaleph.com"
                                target="_blank"
                                rel="noopener"
                                class="text-primary hover:underline"
                            >
                                {t!(i18n, settings.help.docs)}
                            </a>
                        </li>
                        <li>
                            "• "
                            <a
                                href="https://github.com/rootazero/Aleph/issues/"
                                target="_blank"
                                rel="noopener"
                                class="text-primary hover:underline"
                            >
                                {t!(i18n, settings.help.issues)}
                            </a>
                        </li>
                        <li>
                            "• "
                            <a
                                href="mailto:rootazerox@gmail.com"
                                class="text-primary hover:underline"
                            >
                                {t!(i18n, settings.help.support)}
                            </a>
                        </li>
                    </ul>
                </div>
            </section>
        </div>
    }
}

#[component]
fn SetupRow(step: SetupStep, status: Option<bool>) -> impl IntoView {
    let (badge_class, badge_label) = match status {
        Some(true) => ("bg-success/15 text-success", "Ready"),
        Some(false) => ("bg-warning/15 text-warning", "Pending"),
        None => ("bg-surface-sunken text-text-tertiary", "—"),
    };
    view! {
        <div class="flex items-center justify-between gap-4 p-5 bg-surface-raised border border-border rounded-xl hover:border-border-strong transition-colors">
            <div class="flex items-start gap-3 min-w-0">
                <span class=format!("px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wider flex-shrink-0 {}", badge_class)>
                    {badge_label}
                </span>
                <div class="min-w-0">
                    <div class="font-medium text-text-primary text-sm">{step.title}</div>
                    <div class="text-xs text-text-secondary mt-0.5">{step.body}</div>
                </div>
            </div>
            <A
                href=step.href
                attr:class="text-xs font-medium text-primary hover:text-primary-hover whitespace-nowrap flex-shrink-0"
            >
                {step.cta} " →"
            </A>
        </div>
    }
}
