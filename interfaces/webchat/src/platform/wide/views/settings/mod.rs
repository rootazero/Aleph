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
pub mod workspaces;

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
pub use workspaces::WorkspacesView;

// Settings default view (sidebar is provided by SettingsLayout)
use crate::api::{MemoryApi, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
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

/// What a step's live probe actually came back with.
///
/// The fourth variant is the whole point. Every probe here used to collapse
/// `Err(_)` into `Some(false)` — "not configured" — so a member, whose reads
/// are refused by the admin gate, was told `PENDING · Configure a chat
/// provider` about a provider that was configured, and invited to click into a
/// page they cannot use. A refusal is not evidence about the setting; neither
/// is a dropped socket. Only [`Self::Pending`] claims something is missing, and
/// only a successful read may produce it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StepStatus {
    /// Not asked yet (disconnected), or asked and the answer never arrived.
    Unknown,
    Ready,
    Pending,
    /// The server refused the read for lack of operator privilege.
    Restricted,
}

impl StepStatus {
    /// Classify one probe. `Ok` is the only input that may claim anything about
    /// the setting itself.
    fn of(probe: Result<bool, String>) -> Self {
        match probe {
            Ok(true) => Self::Ready,
            Ok(false) => Self::Pending,
            Err(e) if crate::components::admin_refusal::is_admin_refusal(&e) => Self::Restricted,
            // A transport failure, a store error, a malformed response: none of
            // these say the provider is missing either. They used to.
            Err(_) => Self::Unknown,
        }
    }
}

#[component]
#[must_use]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    // Live status. Each probe is classified through `StepStatus::of`, which is
    // what keeps a refusal (or any other failure) from being reported as
    // "not configured".
    let providers_ready = RwSignal::new(StepStatus::Unknown);
    let generation_ready = RwSignal::new(StepStatus::Unknown);
    let memory_ready = RwSignal::new(StepStatus::Unknown);
    let moa_ready = RwSignal::new(StepStatus::Unknown);

    Effect::new(move || {
        if !state.is_connected.get() {
            providers_ready.set(StepStatus::Unknown);
            generation_ready.set(StepStatus::Unknown);
            memory_ready.set(StepStatus::Unknown);
            moa_ready.set(StepStatus::Unknown);
            return;
        }
        leptos::task::spawn_local(async move {
            // Step 1 — at least one chat provider configured.
            providers_ready.set(StepStatus::of(
                ProvidersApi::list(&state).await.map(|l| !l.is_empty()),
            ));

            // Step 2 — at least one generation provider configured.
            generation_ready.set(StepStatus::of(
                crate::api::generation_providers::GenerationProvidersApi::list(&state)
                    .await
                    .map(|l| !l.is_empty()),
            ));

            // Step 3 — memory backend online (any stat row returns).
            memory_ready.set(StepStatus::of(
                MemoryApi::stats(&state, "main").await.map(|_| true),
            ));

            // Step 4 — at least one MoA preset configured (optional).
            moa_ready.set(StepStatus::of(
                crate::api::moa::MoaApi::list_presets(&state)
                    .await
                    .map(|cfg| !cfg.presets.is_empty()),
            ));
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
                        let all = [providers_ready.get(), generation_ready.get(), memory_ready.get(), moa_ready.get()];
                        let done = all.iter().filter(|s| **s == StepStatus::Ready).count();
                        let restricted = all.iter().filter(|s| **s == StepStatus::Restricted).count();
                        let total = all.len();
                        // A refused step is NOT counted as "not ready" — the
                        // counter would then read as a to-do list for work the
                        // reader cannot do and may not even need to.
                        let suffix = if restricted > 0 {
                            format!(" · {restricted} restricted")
                        } else {
                            String::new()
                        };
                        view! {
                            <span class="text-xs font-mono text-text-tertiary">
                                {format!("{done}/{total} ready{suffix}")}
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
fn SetupRow(step: SetupStep, status: StepStatus) -> impl IntoView {
    let i18n = use_i18n();
    let (badge_class, badge_label) = match status {
        StepStatus::Ready => ("bg-success/15 text-success", "Ready"),
        StepStatus::Pending => ("bg-warning/15 text-warning", "Pending"),
        StepStatus::Restricted => ("bg-surface-sunken text-text-tertiary", "Restricted"),
        StepStatus::Unknown => ("bg-surface-sunken text-text-tertiary", "—"),
    };
    let restricted = status == StepStatus::Restricted;
    // A refused step says why, in place of the step's own advice — that advice
    // ("add at least one LLM provider") is a claim about configuration this
    // connection was not allowed to read.
    let body = if restricted {
        t_string!(i18n, settings.admin_refusal.read_setup_state).to_string()
    } else {
        step.body.to_string()
    };
    view! {
        <div class="flex items-center justify-between gap-4 p-5 bg-surface-raised border border-border rounded-xl hover:border-border-strong transition-colors">
            <div class="flex items-start gap-3 min-w-0">
                <span class=format!("px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wider flex-shrink-0 {}", badge_class)>
                    {badge_label}
                </span>
                <div class="min-w-0">
                    <div class="font-medium text-text-primary text-sm">{step.title}</div>
                    <div class="text-xs text-text-secondary mt-0.5">{body}</div>
                </div>
            </div>
            // No call to action on a refused step: the link led to a page this
            // connection cannot use, so offering it was the second half of the
            // same false claim.
            <Show when=move || !restricted>
                <A
                    href=step.href
                    attr:class="text-xs font-medium text-primary hover:text-primary-hover whitespace-nowrap flex-shrink-0"
                >
                    {step.cta} " →"
                </A>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::StepStatus;
    use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

    /// The bug this checklist shipped with: a refused read rendered as
    /// `PENDING · Configure a chat provider` for a provider that was in fact
    /// configured, next to a link into a page the reader cannot open.
    #[test]
    fn a_refused_probe_is_never_reported_as_not_configured() {
        let status = StepStatus::of(Err(format!(
            "Failed to load providers: {ADMIN_REQUIRED_MESSAGE}"
        )));
        assert!(
            status == StepStatus::Restricted,
            "a refusal must not be classified as Pending — Pending is a claim \
             about the setting, and this read never saw the setting"
        );
    }

    /// A refusal is not the only failure that says nothing about the setting.
    #[test]
    fn no_failure_at_all_is_evidence_that_something_is_unconfigured() {
        for err in [
            "Not connected",
            "WebSocket disconnected",
            "Invalid response",
        ] {
            assert!(
                StepStatus::of(Err(err.to_string())) == StepStatus::Unknown,
                "{err} must not claim the step is Pending"
            );
        }
    }

    /// ...and the successful reads still say exactly what they used to, so the
    /// operator's checklist is unchanged.
    #[test]
    fn a_successful_probe_still_answers_ready_or_pending() {
        assert!(StepStatus::of(Ok(true)) == StepStatus::Ready);
        assert!(StepStatus::of(Ok(false)) == StepStatus::Pending);
    }
}
