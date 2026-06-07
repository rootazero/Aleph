//! Gateway-security toggles + network access + paired-device management.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{DeviceInfo, SecurityConfig, SecurityConfigApi};
use crate::context::DashboardState;
use crate::i18n::*;

#[component]
pub(super) fn GatewaySecuritySettings(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
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
pub(super) fn NetworkAccessSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();

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
            </div>
        </div>
    }
}

#[component]
pub(super) fn PairedDevices(
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
    let set_level = move |device_id: String, level: String| {
        spawn_local(async move {
            match SecurityConfigApi::set_level(&state, device_id, level).await {
                Ok(_) => {
                    if let Ok(devs) = SecurityConfigApi::list_devices(&state).await {
                        devices.set(devs);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to set device level: {}", e).into(),
                    );
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
                                    let device_id_sl = device.device_id.clone();
                                    view! {
                                        <DeviceCard
                                            device=device
                                            on_revoke=move || revoke_device(device_id.clone())
                                            on_set_level=move |level: String| set_level(device_id_sl.clone(), level)
                                        />
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
pub(super) fn DeviceCard<F, G>(device: DeviceInfo, on_revoke: F, on_set_level: G) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn(String) + 'static,
{
    let i18n = use_i18n();
    let paired_date = device.paired_at.clone();
    let last_seen_text = device
        .last_seen
        .clone()
        .unwrap_or_else(|| t_string!(i18n, settings.security.never).to_string());
    let is_config = device.tier == "config";
    let target_level = if is_config { "chat" } else { "config" };

    view! {
        <div class="flex items-center justify-between p-4 bg-surface-sunken rounded border border-border">
            <div class="flex-1">
                <div class="font-medium flex items-center gap-2">
                    {device.device_name}
                    <span class=move || {
                        if is_config {
                            "text-xs px-1.5 py-0.5 rounded bg-indigo-600 text-white"
                        } else {
                            "text-xs px-1.5 py-0.5 rounded bg-surface-raised text-text-secondary"
                        }
                    }>
                        {move || if is_config {
                            t_string!(i18n, settings.security.tier_config).to_string()
                        } else {
                            t_string!(i18n, settings.security.tier_chat).to_string()
                        }}
                    </span>
                </div>
                <div class="text-sm text-text-tertiary">
                    {device.device_type} " • " {device.device_id}
                </div>
                <div class="text-xs text-text-secondary mt-1">
                    {t!(i18n, settings.security.paired)} ": " {paired_date} " • " {t!(i18n, settings.security.last_seen)} ": " {last_seen_text}
                </div>
            </div>
            <div class="flex items-center gap-2">
                <button
                    on:click=move |_| on_set_level(target_level.to_string())
                    class="px-3 py-1 bg-surface-raised text-text-primary text-sm rounded hover:bg-surface-sunken border border-border"
                >
                    {move || if is_config {
                        t_string!(i18n, settings.security.downgrade_chat).to_string()
                    } else {
                        t_string!(i18n, settings.security.grant_config).to_string()
                    }}
                </button>
                <button
                    on:click=move |_| on_revoke()
                    class="px-3 py-1 bg-danger text-white text-sm rounded hover:bg-danger"
                >
                    {t!(i18n, settings.security.revoke)}
                </button>
            </div>
        </div>
    }
}
