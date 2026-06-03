//! Local/cloud route-mode settings page.
//!
//! Three-state selector (Auto / Always Local / Always Cloud) over the backend
//! `[route]` engine. The mode hot-applies on save — the next prompt routes the
//! new way with no daemon restart. The configured providers are shown grouped
//! by the tier the *server* assigned (local vs cloud), so the user sees exactly
//! which endpoints each mode will target without re-deriving locality here.

use crate::api::{RouteConfigApi, RouteConfigUpdate, RouteProviderInfo};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The three selectable modes with their display copy.
const MODES: &[(&str, &str, &str)] = &[
    (
        "auto",
        "Auto",
        "Keep the configured failover order. The smart default — local and cloud are peers, ordered by your provider config.",
    ),
    (
        "always_local",
        "Always Local",
        "Only on-machine endpoints (e.g. http://localhost:11434). Cloud is dropped — for offline or sensitive work.",
    ),
    (
        "always_cloud",
        "Always Cloud",
        "Prefer public-API flagship models. Local stays as an ungated last-resort degrade.",
    ),
];

#[component]
pub fn RouteView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let mode = RwSignal::new(String::from("auto"));
    let allow_escalation = RwSignal::new(false);
    let providers = RwSignal::new(Vec::<RouteProviderInfo>::new());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load on mount.
    {
        spawn_local(async move {
            match RouteConfigApi::get(&state).await {
                Ok(view) => {
                    mode.set(view.mode);
                    allow_escalation.set(view.allow_cloud_escalation);
                    providers.set(view.providers);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    }

    let save = move |_| {
        let state = state;
        saving.set(true);
        saved.set(false);
        error.set(None);
        spawn_local(async move {
            let update = RouteConfigUpdate {
                mode: mode.get(),
                allow_cloud_escalation: allow_escalation.get(),
            };
            match RouteConfigApi::update(&state, update).await {
                Ok(()) => {
                    saving.set(false);
                    saved.set(true);
                }
                Err(e) => {
                    error.set(Some(e));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto">
            <h1 class="text-2xl font-bold mb-1 text-text-primary">"Model Routing"</h1>
            <p class="text-sm text-text-secondary mb-6">
                "Choose how requests split between on-machine and cloud models. \
                 Switching takes effect on your next message — no restart."
            </p>

            <Show when=move || loading.get()>
                <p class="text-text-secondary">"Loading…"</p>
            </Show>

            <Show when=move || !loading.get()>
                <div class="space-y-6">
                    // Mode selector cards
                    <div class="space-y-3">
                        {MODES.iter().map(|(key, label, desc)| {
                            let key = *key;
                            let label = *label;
                            let desc = *desc;
                            let selected = Signal::derive(move || mode.get() == key);
                            view! {
                                <button
                                    on:click=move |_| { mode.set(key.to_string()); saved.set(false); }
                                    class=move || if selected.get() {
                                        "w-full text-left p-4 rounded-lg border-2 border-accent-primary bg-accent-primary/10 transition-colors"
                                    } else {
                                        "w-full text-left p-4 rounded-lg border border-border bg-surface-raised hover:border-border-strong transition-colors"
                                    }
                                >
                                    <div class="flex items-center gap-2 mb-1">
                                        <span class=move || if selected.get() {
                                            "w-3 h-3 rounded-full bg-accent-primary"
                                        } else {
                                            "w-3 h-3 rounded-full border border-border-strong"
                                        }></span>
                                        <span class="font-semibold text-text-primary">{label}</span>
                                    </div>
                                    <p class="text-sm text-text-secondary ml-5">{desc}</p>
                                </button>
                            }
                        }).collect::<Vec<_>>()}
                    </div>

                    // Cloud-escalation toggle (only meaningful in Always Local)
                    <Show when=move || mode.get() == "always_local">
                        <div class="bg-surface-raised rounded-lg border border-border p-4 flex items-start gap-3">
                            <input
                                type="checkbox"
                                class="mt-1"
                                prop:checked=move || allow_escalation.get()
                                on:change=move |ev| {
                                    allow_escalation.set(event_target_checked(&ev));
                                    saved.set(false);
                                }
                            />
                            <div>
                                <div class="font-medium text-text-primary">"Allow borrowing cloud (approval-gated)"</div>
                                <p class="text-sm text-text-secondary">
                                    "If no local provider succeeds, ask before trying a cloud provider as a last resort. \
                                     Off: cloud is never used in this mode."
                                </p>
                            </div>
                        </div>
                    </Show>

                    // Save
                    <div class="flex items-center gap-3">
                        <button
                            class="px-4 py-2 bg-accent-primary text-white rounded-lg hover:bg-accent-primary/90 disabled:opacity-50"
                            disabled=move || saving.get()
                            on:click=save
                        >
                            {move || if saving.get() { "Saving…" } else { "Apply" }}
                        </button>
                        <Show when=move || saved.get()>
                            <span class="text-sm text-green-500">"Applied — your next message uses this route."</span>
                        </Show>
                    </div>

                    <Show when=move || error.get().is_some()>
                        <div class="text-red-500 text-sm">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    // Configured providers, grouped by server-assigned tier.
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4 pt-2">
                        <ProviderTierCard
                            title="Local endpoints".to_string()
                            tier="local".to_string()
                            providers=providers
                        />
                        <ProviderTierCard
                            title="Cloud endpoints".to_string()
                            tier="cloud".to_string()
                            providers=providers
                        />
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// One tier column listing the configured providers the server placed in it.
#[component]
fn ProviderTierCard(
    title: String,
    tier: String,
    providers: RwSignal<Vec<RouteProviderInfo>>,
) -> impl IntoView {
    let tier_for_filter = tier.clone();
    let matching = Signal::derive(move || {
        providers
            .get()
            .into_iter()
            .filter(|p| p.tier == tier_for_filter)
            .collect::<Vec<_>>()
    });

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-4">
            <h3 class="font-semibold text-text-primary mb-2">{title}</h3>
            <Show
                when=move || !matching.get().is_empty()
                fallback=move || view! {
                    <p class="text-sm text-text-tertiary">"No configured providers in this tier."</p>
                }
            >
                <ul class="space-y-1">
                    {move || matching.get().into_iter().map(|p| {
                        let models = if p.models.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", p.models.join(", "))
                        };
                        let dim = !p.enabled;
                        view! {
                            <li class=move || if dim {
                                "text-sm text-text-tertiary"
                            } else {
                                "text-sm text-text-secondary"
                            }>
                                <span class="font-mono text-text-primary">{p.name.clone()}</span>
                                {models}
                                {(!p.enabled).then(|| view! { <span class="ml-1 text-text-tertiary">"(disabled)"</span> })}
                            </li>
                        }
                    }).collect::<Vec<_>>()}
                </ul>
            </Show>
        </div>
    }
}
