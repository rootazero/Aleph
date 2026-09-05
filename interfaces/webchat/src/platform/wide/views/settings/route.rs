//! Local/cloud route-mode settings page.
//!
//! Three-state selector (Auto / Always Local / Always Cloud) over the backend
//! `[route]` engine. The mode hot-applies on save — the next prompt routes the
//! new way with no daemon restart. The configured providers are shown grouped
//! by the tier the *server* assigned (local vs cloud), so the user sees exactly
//! which endpoints each mode will target without re-deriving locality here.

use crate::api::{
    parse_probe_interval as parse_interval, RateLimit, RouteConfigApi, RouteConfigUpdate,
    RouteProviderInfo,
};
use crate::components::route_labels::{lb_label, mode_desc, mode_label, LB_KEYS, MODE_KEYS};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::BTreeMap;

#[component]
#[must_use]
pub fn RouteView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let mode = RwSignal::new(String::from("auto"));
    let allow_escalation = RwSignal::new(false);
    // Empty string == "no pin / configured order"; a provider name == that pin.
    let local_provider = RwSignal::new(String::new());
    let cloud_provider = RwSignal::new(String::new());
    let providers = RwSignal::new(Vec::<RouteProviderInfo>::new());
    let load_balance = RwSignal::new(String::from("ordered"));
    let rate_limits = RwSignal::new(BTreeMap::<String, RateLimit>::new());
    // Health-probe interval as typed text; empty == off. Seeded from the view
    // and always sent back: `route_config.update` full-replaces `[route]`, so
    // dropping the key would switch off a prober the operator enabled in TOML.
    let probe_interval = RwSignal::new(String::new());
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
                    local_provider.set(view.local_provider.unwrap_or_default());
                    cloud_provider.set(view.cloud_provider.unwrap_or_default());
                    providers.set(view.providers);
                    load_balance.set(view.load_balance.unwrap_or_else(|| "ordered".into()));
                    rate_limits.set(view.rate_limits);
                    probe_interval.set(
                        view.health_probe_interval_secs
                            .map(|s| s.to_string())
                            .unwrap_or_default(),
                    );
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
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
            // Empty selection clears the pin (server normalises blank → None).
            let to_pin = |s: String| if s.is_empty() { None } else { Some(s) };
            let update = RouteConfigUpdate {
                mode: mode.get(),
                allow_cloud_escalation: allow_escalation.get(),
                local_provider: to_pin(local_provider.get()),
                cloud_provider: to_pin(cloud_provider.get()),
                load_balance: Some(load_balance.get()),
                rate_limits: rate_limits.get(),
                health_probe_interval_secs: parse_interval(&probe_interval.get()),
            };
            match RouteConfigApi::update(&state, update).await {
                Ok(()) => {
                    saving.set(false);
                    saved.set(true);
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto">
            <h1 class="text-2xl font-bold mb-1 text-text-primary">{t!(i18n, settings.route.title)}</h1>
            <p class="text-sm text-text-secondary mb-6">
                {t!(i18n, settings.route.description)}
            </p>

            <Show when=move || loading.get()>
                <p class="text-text-secondary">{t!(i18n, settings.route.loading)}</p>
            </Show>

            <Show when=move || !loading.get()>
                <div class="space-y-6">
                    // Mode selector cards
                    <div class="space-y-3">
                        {MODE_KEYS.iter().map(|key| {
                            let key = *key;
                            let label = move || mode_label(i18n, key);
                            let desc = move || mode_desc(i18n, key);
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

                    // Load-balancing strategy — how same-tier providers are
                    // ordered within the active route. Default "ordered" is a
                    // no-op (configured order).
                    <div class="bg-surface-raised rounded-lg border border-border p-4">
                        <label class="block font-semibold text-text-primary mb-1">
                            {t!(i18n, settings.route.load_balance)}
                        </label>
                        <p class="text-sm text-text-secondary mb-2">
                            {t!(i18n, settings.route.load_balance_desc)}
                        </p>
                        <select
                            class="w-full bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text-primary"
                            prop:value=move || load_balance.get()
                            on:change=move |ev| {
                                load_balance.set(event_target_value(&ev));
                                saved.set(false);
                            }
                        >
                            {LB_KEYS.iter().map(|key| {
                                let key = *key;
                                let label = move || lb_label(i18n, key);
                                view! { <option value=key>{label}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>

                    // Background health probe — how often a circuit-open
                    // provider is re-dialled. Blank / 0 = off (the default);
                    // a probe is a real, paid request.
                    <div class="bg-surface-raised rounded-lg border border-border p-4">
                        <label class="block font-semibold text-text-primary mb-1">
                            {t!(i18n, settings.route.health_probe)}
                        </label>
                        <p class="text-sm text-text-secondary mb-2">
                            {t!(i18n, settings.route.health_probe_desc)}
                        </p>
                        <input
                            type="number"
                            min="0"
                            class="w-40 bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text-primary"
                            placeholder=move || t_string!(i18n, settings.route.health_probe_off).to_string()
                            prop:value=move || probe_interval.get()
                            on:input=move |ev| {
                                probe_interval.set(event_target_value(&ev));
                                saved.set(false);
                            }
                        />
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
                                <div class="font-medium text-text-primary">{t!(i18n, settings.route.allow_escalation)}</div>
                                <p class="text-sm text-text-secondary">
                                    {t!(i18n, settings.route.allow_escalation_desc)}
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
                            {move || if saving.get() {
                                t_string!(i18n, settings.route.saving).to_string()
                            } else {
                                t_string!(i18n, settings.route.apply).to_string()
                            }}
                        </button>
                        <Show when=move || saved.get()>
                            <span class="text-sm text-green-500">{t!(i18n, settings.route.applied)}</span>
                        </Show>
                    </div>

                    <Show when=move || error.get().is_some()>
                        <div class="text-red-500 text-sm">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    // Preferred providers — pick which configured local/cloud
                    // endpoint the active route dials first. The dropdowns reuse
                    // the already-configured providers (nothing is redefined here),
                    // satisfying the "select from configured provider/model" ask.
                    <div class="pt-2">
                        <h3 class="font-semibold text-text-primary mb-1">{t!(i18n, settings.route.preferred_providers)}</h3>
                        <p class="text-sm text-text-secondary mb-3">
                            {t!(i18n, settings.route.preferred_providers_desc)}
                        </p>
                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <ProviderTierSelect
                                tier="local".to_string()
                                providers=providers
                                selected=local_provider
                            />
                            <ProviderTierSelect
                                tier="cloud".to_string()
                                providers=providers
                                selected=cloud_provider
                            />
                        </div>
                    </div>
                    // Per-provider rpm/tpm ceilings (used by Usage-based).
                    <RateLimitEditor
                        providers=providers
                        rate_limits=rate_limits
                        saved=saved
                    />
                </div>
            </Show>
        </div>
    }
}

/// One tier's preferred-provider dropdown, populated from the configured
/// providers the server placed in that tier. The empty option == "no pin"
/// (configured order). Selecting a provider pins it; the change is applied on
/// the page's "Apply" button alongside the mode.
#[component]
fn ProviderTierSelect(
    tier: String,
    providers: RwSignal<Vec<RouteProviderInfo>>,
    selected: RwSignal<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let is_local = tier == "local";
    let tier_for_filter = tier;
    let matching = Signal::derive(move || {
        providers
            .get()
            .into_iter()
            .filter(|p| p.tier == tier_for_filter)
            .collect::<Vec<_>>()
    });

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-4">
            <label class="block font-semibold text-text-primary mb-2">
                {move || if is_local {
                    t_string!(i18n, settings.route.local_provider).to_string()
                } else {
                    t_string!(i18n, settings.route.cloud_provider).to_string()
                }}
            </label>
            <select
                class="w-full bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text-primary"
                prop:value=move || selected.get()
                on:change=move |ev| selected.set(event_target_value(&ev))
            >
                <option value="">{t!(i18n, settings.route.configured_order)}</option>
                {move || matching.get().into_iter().map(|p| {
                    let label = if p.models.is_empty() {
                        p.name.clone()
                    } else {
                        format!("{} · {}", p.name, p.models.join(", "))
                    };
                    let suffix = if p.enabled { "" } else { " (disabled)" };
                    view! {
                        <option value=p.name>{label}{suffix}</option>
                    }
                }).collect::<Vec<_>>()}
            </select>
            <Show when=move || matching.get().is_empty()>
                <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.route.no_providers)}</p>
            </Show>
        </div>
    }
}

/// Per-provider soft rate-limit editor. Iterates every configured provider and
/// exposes two optional number inputs (rpm / tpm). An empty field clears that
/// dimension; clearing both removes the provider's entry entirely, so the saved
/// `rate_limits` map stays minimal and byte-identical to a hand-written
/// `[route.rate_limits.*]` block.
#[component]
fn RateLimitEditor(
    providers: RwSignal<Vec<RouteProviderInfo>>,
    rate_limits: RwSignal<BTreeMap<String, RateLimit>>,
    saved: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="pt-2">
            <h3 class="font-semibold text-text-primary mb-1">{t!(i18n, settings.route.rate_limits)}</h3>
            <p class="text-sm text-text-secondary mb-3">
                {t!(i18n, settings.route.rate_limits_desc)}
            </p>
            <div class="space-y-2">
                {move || providers.get().into_iter().map(|p| {
                    let name = p.name.clone();
                    let name_rpm = name.clone();
                    let name_tpm = name.clone();
                    let rpm_val = {
                        let name = name.clone();
                        move || rate_limits.get().get(&name).and_then(|r| r.rpm)
                            .map(|v| v.to_string()).unwrap_or_default()
                    };
                    let tpm_val = {
                        let name = name;
                        move || rate_limits.get().get(&name).and_then(|r| r.tpm)
                            .map(|v| v.to_string()).unwrap_or_default()
                    };
                    view! {
                        <div class="flex items-center gap-3 bg-surface-raised rounded-lg border border-border p-3">
                            <span class="flex-1 text-sm text-text-primary truncate">{p.name}</span>
                            <input
                                type="number"
                                min="0"
                                class="w-28 bg-surface border border-border rounded px-2 py-1 text-sm text-text-primary"
                                title=move || t_string!(i18n, settings.route.rpm).to_string()
                                placeholder=move || t_string!(i18n, settings.route.unlimited).to_string()
                                prop:value=rpm_val
                                on:input=move |ev| {
                                    let v = parse_limit(&event_target_value(&ev));
                                    let key = name_rpm.clone();
                                    rate_limits.update(|m| {
                                        let e = m.entry(key.clone()).or_default();
                                        e.rpm = v;
                                        if e.rpm.is_none() && e.tpm.is_none() { m.remove(&key); }
                                    });
                                    saved.set(false);
                                }
                            />
                            <input
                                type="number"
                                min="0"
                                class="w-28 bg-surface border border-border rounded px-2 py-1 text-sm text-text-primary"
                                title=move || t_string!(i18n, settings.route.tpm).to_string()
                                placeholder=move || t_string!(i18n, settings.route.unlimited).to_string()
                                prop:value=tpm_val
                                on:input=move |ev| {
                                    let v = parse_limit(&event_target_value(&ev));
                                    let key = name_tpm.clone();
                                    rate_limits.update(|m| {
                                        let e = m.entry(key.clone()).or_default();
                                        e.tpm = v;
                                        if e.rpm.is_none() && e.tpm.is_none() { m.remove(&key); }
                                    });
                                    saved.set(false);
                                }
                            />
                        </div>
                    }
                }).collect::<Vec<_>>()}
                <Show when=move || providers.get().is_empty()>
                    <p class="text-xs text-text-tertiary">{t!(i18n, settings.route.no_providers)}</p>
                </Show>
            </div>
        </div>
    }
}

/// Parse a number-input string into an optional ceiling. Empty / non-numeric →
/// `None` (that dimension is unbounded). Mirrors the backend's "omitted = no
/// limit" contract.
fn parse_limit(raw: &str) -> Option<u32> {
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<u32>().ok()
    }
}
