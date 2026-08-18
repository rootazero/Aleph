//! iPhone Model-route detail screen.
//!
//! Mirrors the data contract of `platform/wide/views/settings/route.rs` exactly
//! — same signals, same `RouteConfigApi::get` load, same `RouteConfigApi::update`
//! save closure, same `parse_limit` helper. Only the presentation changes to iOS
//! list/cell/toggle/inline-input idioms (R4 pure I/O, R2 single UI truth).

use crate::api::{RateLimit, RouteConfigApi, RouteConfigUpdate, RouteProviderInfo};
use crate::context::DashboardState;
use crate::i18n::{t, t_string};
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::BTreeMap;

/// The three selectable mode keys — copied verbatim from route.rs.
const MODE_KEYS: &[&str] = &["auto", "always_local", "always_cloud"];

/// Load-balancing strategy keys — copied verbatim from route.rs.
const LB_KEYS: &[&str] = &[
    "ordered",
    "round_robin",
    "least_busy",
    "latency_aware",
    "usage_based",
    "cost_aware",
];

/// Parse a number-input string into an optional ceiling — copied verbatim from route.rs.
fn parse_limit(raw: &str) -> Option<u32> {
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<u32>().ok()
    }
}

#[component]
#[must_use]
pub fn PhoneModelRoute() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let state = expect_context::<DashboardState>();

    // --- signals: identical set to route.rs ---
    let mode = RwSignal::new(String::from("auto"));
    let allow_escalation = RwSignal::new(false);
    let local_provider = RwSignal::new(String::new());
    let cloud_provider = RwSignal::new(String::new());
    let providers = RwSignal::new(Vec::<RouteProviderInfo>::new());
    let load_balance = RwSignal::new(String::from("ordered"));
    let rate_limits = RwSignal::new(BTreeMap::<String, RateLimit>::new());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load once the socket connects; recovers after cold-boot / reconnect
    // (rpc_call returns "Not connected" until the WS handshake completes, so a
    // bare mount-time spawn strands a permanent error). Logic mirrors route.rs.
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                error.set(None);
                match RouteConfigApi::get(&state).await {
                    Ok(view) => {
                        mode.set(view.mode);
                        allow_escalation.set(view.allow_cloud_escalation);
                        local_provider.set(view.local_provider.unwrap_or_default());
                        cloud_provider.set(view.cloud_provider.unwrap_or_default());
                        providers.set(view.providers);
                        load_balance.set(view.load_balance.unwrap_or_else(|| "ordered".into()));
                        rate_limits.set(view.rate_limits);
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
    });

    // Save closure — identical logic to route.rs.
    let save = move |_| {
        let state = state;
        saving.set(true);
        saved.set(false);
        error.set(None);
        spawn_local(async move {
            let to_pin = |s: String| if s.is_empty() { None } else { Some(s) };
            let update = RouteConfigUpdate {
                mode: mode.get(),
                allow_cloud_escalation: allow_escalation.get(),
                local_provider: to_pin(local_provider.get()),
                cloud_provider: to_pin(cloud_provider.get()),
                load_balance: Some(load_balance.get()),
                rate_limits: rate_limits.get(),
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
        <PhoneShell title="Model route" back="/settings">
            // "Apply" apply button positioned in the top-right corner of the body
            // (outside the shell's top-bar slot; sits at the scroll-body top).
            <div style="display:flex; justify-content:flex-end; padding:0 14px 8px;">
                <button
                    style="background:none; border:0; cursor:pointer; color:var(--color-primary); font:inherit; font-size:16px; padding:4px 0;"
                    disabled=move || saving.get()
                    on:click=save
                >
                    {move || if saving.get() { t_string!(i18n, settings.route.saving).to_string() } else { t_string!(i18n, settings.route.apply).to_string() }}
                </button>
            </div>

            // Feedback row
            <Show when=move || saved.get()>
                <div style="padding:0 16px 6px; font-size:13px; color:oklch(0.60 0.15 142);">{t!(i18n, settings.phone.applied)}</div>
            </Show>
            <Show when=move || error.get().is_some()>
                <div style="padding:0 16px 6px; font-size:13px; color:oklch(0.55 0.20 27);">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            // Loading state
            <Show when=move || loading.get()>
                <div style="padding:0 16px; font-size:14px; color:var(--color-text-secondary);">{t!(i18n, settings.route.loading)}</div>
            </Show>

            <Show when=move || !loading.get()>
                // ① Mode — 3-row single-select list
                <div>
                    <div class="list-header">{t!(i18n, settings.phone.mode)}</div>
                    <div class="list">
                        {MODE_KEYS.iter().map(|key| {
                            let key = *key;
                            let label = match key {
                                "auto" => "Auto",
                                "always_local" => "Always Local",
                                _ => "Always Cloud",
                            };
                            view! {
                                <div
                                    class="cell"
                                    class:cell-selected=move || mode.get() == key
                                    on:click=move |_| { mode.set(key.to_string()); saved.set(false); }
                                >
                                    <div class="cell-body"><div class="cell-title">{label}</div></div>
                                    <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                                        <polyline points="20 6 9 17 4 12"></polyline>
                                    </svg>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </div>

                // ② Load Balancing — single-select list over LB_KEYS
                <div>
                    <div class="list-header">{t!(i18n, settings.route.load_balance)}</div>
                    <div class="list">
                        {LB_KEYS.iter().map(|key| {
                            let key = *key;
                            let label = match key {
                                "ordered" => "Ordered",
                                "round_robin" => "Round Robin",
                                "least_busy" => "Least Busy",
                                "latency_aware" => "Latency Aware",
                                "usage_based" => "Usage Based",
                                _ => "Cost Aware",
                            };
                            view! {
                                <div
                                    class="cell"
                                    class:cell-selected=move || load_balance.get() == key
                                    on:click=move |_| { load_balance.set(key.to_string()); saved.set(false); }
                                >
                                    <div class="cell-body"><div class="cell-title">{label}</div></div>
                                    <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                                        <polyline points="20 6 9 17 4 12"></polyline>
                                    </svg>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </div>

                // ③ Cloud Escalation toggle — only when mode == "always_local"
                <Show when=move || mode.get() == "always_local">
                    <div>
                        <div class="list-header">{t!(i18n, settings.phone.escalation)}</div>
                        <div class="list">
                            <div class="cell">
                                <div class="cell-body"><div class="cell-title">{t!(i18n, settings.phone.allow_escalation)}</div></div>
                                <button
                                    class="ios-switch"
                                    attr:aria-pressed=move || allow_escalation.get().to_string()
                                    on:click=move |_| allow_escalation.update(|v| *v = !*v)
                                >
                                    <span class="ios-knob"></span>
                                </button>
                            </div>
                        </div>
                    </div>
                </Show>

                // ④ Preferred providers — local + cloud single-select lists
                <ProviderPinGroup
                    header=Signal::derive(move || t_string!(i18n, settings.phone.local_providers).to_string())
                    tier="local".to_string()
                    providers=providers
                    selected=local_provider
                    on_change=move |_| saved.set(false)
                />
                <ProviderPinGroup
                    header=Signal::derive(move || t_string!(i18n, settings.phone.cloud_providers).to_string())
                    tier="cloud".to_string()
                    providers=providers
                    selected=cloud_provider
                    on_change=move |_| saved.set(false)
                />

                // ⑤ Rate Limit — per-provider rpm/tpm inline number inputs
                <Show when=move || !providers.get().is_empty()>
                    <div>
                        <div class="list-header">{t!(i18n, settings.route.rate_limits)}</div>
                        <div class="list">
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
                                    <div class="cell" style="flex-direction:column; align-items:stretch; gap:6px; padding:10px 16px;">
                                        <div class="cell-title" style="font-size:13px; color:var(--color-text-primary);">{p.name}</div>
                                        <div style="display:flex; gap:8px; align-items:center;">
                                            <input
                                                type="number"
                                                min="0"
                                                style="flex:1; background:var(--color-surface); border:1px solid var(--color-border); border-radius:8px; padding:6px 10px; font-size:13px; color:var(--color-text-primary);"
                                                placeholder=move || t_string!(i18n, settings.phone.rpm_placeholder).to_string()
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
                                                style="flex:1; background:var(--color-surface); border:1px solid var(--color-border); border-radius:8px; padding:6px 10px; font-size:13px; color:var(--color-text-primary);"
                                                placeholder=move || t_string!(i18n, settings.phone.tpm_placeholder).to_string()
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
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                </Show>
            </Show>
        </PhoneShell>
    }
}

/// One tier's preferred-provider single-select list.
/// Empty selection = "Configured Order" (no pin / use configured order).
#[component]
fn ProviderPinGroup<F>(
    header: Signal<String>,
    tier: String,
    providers: RwSignal<Vec<RouteProviderInfo>>,
    selected: RwSignal<String>,
    on_change: F,
) -> impl IntoView
where
    F: Fn(()) + Copy + Send + 'static,
{
    let i18n = crate::i18n::use_i18n();
    let tier_for_filter = tier;
    let matching = Signal::derive(move || {
        providers
            .get()
            .into_iter()
            .filter(|p| p.tier == tier_for_filter)
            .collect::<Vec<_>>()
    });

    view! {
        <div>
            <div class="list-header">{header}</div>
            <div class="list">
                // "Configured Order" = empty / no pin
                <div
                    class="cell"
                    class:cell-selected=move || selected.get().is_empty()
                    on:click=move |_| { selected.set(String::new()); on_change(()); }
                >
                    <div class="cell-body"><div class="cell-title">{t!(i18n, settings.phone.configured_order)}</div></div>
                    <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="20 6 9 17 4 12"></polyline>
                    </svg>
                </div>
                {move || matching.get().into_iter().map(|p| {
                    let name = p.name.clone();
                    let name_for_select = name.clone();
                    let label = if p.models.is_empty() {
                        p.name.clone()
                    } else {
                        format!("{} · {}", p.name, p.models.join(", "))
                    };
                    let suffix = if p.enabled { "" } else { " (disabled)" };
                    let full_label = format!("{}{}", label, suffix);
                    view! {
                        <div
                            class="cell"
                            class:cell-selected=move || selected.get() == name
                            on:click=move |_| { selected.set(name_for_select.clone()); on_change(()); }
                        >
                            <div class="cell-body"><div class="cell-title">{full_label}</div></div>
                            <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="20 6 9 17 4 12"></polyline>
                            </svg>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}
