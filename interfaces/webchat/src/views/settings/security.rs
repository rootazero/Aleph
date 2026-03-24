//! Security Configuration View
//!
//! Provides UI for managing security settings:
//! - Gateway security settings (require auth, enable pairing, allow guest)
//! - Paired devices management
//! - Device revocation
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{SecurityConfigApi, SecurityConfig, DeviceInfo, SearchConfigApi, SearchConfig};
use crate::context::DashboardState;
use crate::i18n::*;

#[component]
pub fn SecurityView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(Option::<SecurityConfig>::None);
    let devices = RwSignal::new(Vec::<DeviceInfo>::new());
    let search_config = RwSignal::new(SearchConfig {
        enabled: false,
        default_provider: String::new(),
        max_results: 5,
        timeout_seconds: 10,
        pii_enabled: false,
        pii_scrub_email: true,
        pii_scrub_phone: true,
        pii_scrub_ssn: true,
        pii_scrub_credit_card: true,
        backends: Vec::new(),
    });
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load config and devices on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);

                // Load security config
                match SecurityConfigApi::get(&state).await {
                    Ok(cfg) => {
                        config.set(Some(cfg));
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load security config: {}", e)));
                    }
                }

                // Load devices
                match SecurityConfigApi::list_devices(&state).await {
                    Ok(devs) => {
                        devices.set(devs);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load devices: {}", e)));
                    }
                }

                // Load PII config (stored in search config)
                match SearchConfigApi::get(&state).await {
                    Ok(cfg) => {
                        search_config.set(cfg);
                    }
                    Err(_) => {
                        // PII defaults are fine
                    }
                }

                loading.set(false);
            });
        } else {
            loading.set(false);
        }
    });

    let save = move |_| {
        if let Some(cfg) = config.get() {
            spawn_local(async move {
                saving.set(true);
                match SecurityConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to save: {}", e)));
                    }
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto">
            <div class="max-w-4xl">
                <h1 class="text-2xl font-bold mb-6">{t!(i18n, settings.security.title)}</h1>

                {move || {
                    if loading.get() {
                        view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle text-danger rounded">
                                        {e}
                                    </div>
                                })}

                                <GatewaySecuritySettings config=config />
                                <NetworkAccessSection config=config />
                                <PIISection config=search_config />
                                <PairedDevices devices=devices state=state />

                                <div class="pt-4 border-t border-border">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-6 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                                    >
                                        {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn GatewaySecuritySettings(
    config: RwSignal<Option<SecurityConfig>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.security.gateway_security)}</h2>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.require_auth).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.require_auth = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.security.require_auth)}</label>
                </div>
                <p class="text-sm text-text-tertiary ml-6">
                    {t!(i18n, settings.security.require_auth_desc)}
                </p>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.enable_pairing).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.enable_pairing = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.security.enable_pairing)}</label>
                </div>
                <p class="text-sm text-text-tertiary ml-6">
                    {t!(i18n, settings.security.enable_pairing_desc)}
                </p>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.allow_guest).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.allow_guest = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.security.allow_guest)}</label>
                </div>
                <p class="text-sm text-text-tertiary ml-6">
                    {t!(i18n, settings.security.allow_guest_desc)}
                </p>
            </div>
        </div>
    }
}

#[component]
fn NetworkAccessSection(
    config: RwSignal<Option<SecurityConfig>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let save_success = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let saving = RwSignal::new(false);
    let needs_restart = RwSignal::new(false);

    let save_network = move |_| {
        if let Some(cfg) = config.get() {
            saving.set(true);
            save_error.set(None);
            spawn_local(async move {
                match SecurityConfigApi::update(&state, cfg).await {
                    Ok(result) => {
                        saving.set(false);
                        // Check if server returned needs_restart
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&result).unwrap_or_default()) {
                            if v.get("needs_restart").and_then(|v| v.as_bool()).unwrap_or(false) {
                                needs_restart.set(true);
                            }
                        }
                        needs_restart.set(true);
                        save_success.set(true);
                        set_timeout(
                            move || { save_success.set(false); },
                            std::time::Duration::from_secs(5),
                        );
                    }
                    Err(e) => {
                        saving.set(false);
                        save_error.set(Some(e));
                    }
                }
            });
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.security.network_access)}</h2>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        {t!(i18n, settings.security.network_scope)}
                    </label>
                    <select
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.network_access = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary"
                    >
                        <option
                            value="localhost"
                            selected=move || config.get().map(|c| c.network_access == "localhost").unwrap_or(true)
                        >
                            {t!(i18n, settings.security.localhost_only)}
                        </option>
                        <option
                            value="allnetworks"
                            selected=move || config.get().map(|c| c.network_access == "allnetworks").unwrap_or(false)
                        >
                            {t!(i18n, settings.security.all_networks)}
                        </option>
                    </select>
                    <p class="text-xs text-text-tertiary mt-1">
                        {move || {
                            let is_all = config.get().map(|c| c.network_access == "allnetworks").unwrap_or(false);
                            if is_all {
                                t_string!(i18n, settings.security.all_networks_desc).to_string()
                            } else {
                                t_string!(i18n, settings.security.localhost_only_desc).to_string()
                            }
                        }}
                    </p>
                </div>

                {move || {
                    if needs_restart.get() {
                        Some(view! {
                            <div class="p-3 bg-warning-subtle border border-warning/20 rounded text-warning text-sm">
                                {t!(i18n, settings.security.restart_required)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {t!(i18n, common.saved)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=save_network
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn PairedDevices(
    devices: RwSignal<Vec<DeviceInfo>>,
    state: DashboardState,
) -> impl IntoView {
    let i18n = use_i18n();
    let revoke_device = move |device_id: String| {
        spawn_local(async move {
            match SecurityConfigApi::revoke_device(&state, device_id.clone()).await {
                Ok(_) => {
                    // Reload devices
                    if let Ok(devs) = SecurityConfigApi::list_devices(&state).await {
                        devices.set(devs);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to revoke device: {}", e).into());
                }
            }
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.security.paired_devices)}</h2>

            <div class="space-y-3">
                {move || {
                    let device_list = devices.get();
                    if device_list.is_empty() {
                        view! {
                            <div class="text-text-tertiary text-center py-4">
                                {t!(i18n, settings.security.no_devices)}
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-2">
                                {device_list.into_iter().map(|device| {
                                    let device_id = device.device_id.clone();
                                    view! {
                                        <DeviceCard device=device on_revoke=move || revoke_device(device_id.clone()) />
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn DeviceCard<F>(
    device: DeviceInfo,
    on_revoke: F,
) -> impl IntoView
where
    F: Fn() + 'static,
{
    let i18n = use_i18n();
    let paired_date = device.paired_at.clone();
    let last_seen_text = device.last_seen.clone()
        .unwrap_or_else(|| t_string!(i18n, settings.security.never).to_string());

    view! {
        <div class="flex items-center justify-between p-4 bg-surface-sunken rounded border border-border">
            <div class="flex-1">
                <div class="font-medium">{device.device_name}</div>
                <div class="text-sm text-text-tertiary">
                    {device.device_type} " • " {device.device_id}
                </div>
                <div class="text-xs text-text-secondary mt-1">
                    {t!(i18n, settings.security.paired)} ": " {paired_date} " • " {t!(i18n, settings.security.last_seen)} ": " {last_seen_text}
                </div>
            </div>
            <button
                on:click=move |_| on_revoke()
                class="px-3 py-1 bg-danger text-white text-sm rounded hover:bg-danger"
            >
                {t!(i18n, settings.security.revoke)}
            </button>
        </div>
    }
}

#[component]
fn PIISection(config: RwSignal<SearchConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let pii_enabled = RwSignal::new(config.get().pii_enabled);
    let scrub_email = RwSignal::new(config.get().pii_scrub_email);
    let scrub_phone = RwSignal::new(config.get().pii_scrub_phone);
    let scrub_ssn = RwSignal::new(config.get().pii_scrub_ssn);
    let scrub_credit_card = RwSignal::new(config.get().pii_scrub_credit_card);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = StoredValue::new(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.pii_enabled = pii_enabled.get();
        cfg.pii_scrub_email = scrub_email.get();
        cfg.pii_scrub_phone = scrub_phone.get();
        cfg.pii_scrub_ssn = scrub_ssn.get();
        cfg.pii_scrub_credit_card = scrub_credit_card.get();

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || {
                            save_success.set(false);
                        },
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.security.pii_protection)}</h2>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || pii_enabled.get()
                        on:change=move |ev| pii_enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.enable_pii)}</div>
                    </div>
                </label>

                <div class="ml-7 space-y-2 border-l-2 border-border pl-4">
                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_email.get()
                            on:change=move |ev| scrub_email.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_email)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_phone.get()
                            on:change=move |ev| scrub_phone.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_phone)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_ssn.get()
                            on:change=move |ev| scrub_ssn.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_ssn)}</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_credit_card.get()
                            on:change=move |ev| scrub_credit_card.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.security.pii_credit_card)}</span>
                    </label>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {t!(i18n, common.saved)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}
