//! Security Configuration View
//!
//! Provides UI for managing security settings:
//! - Gateway security settings (require auth, enable pairing, allow guest)
//! - Paired devices management
//! - Device revocation
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{SecurityConfigApi, SecurityConfig, DeviceInfo};
use crate::context::DashboardState;

#[component]
pub fn SecurityView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = create_rw_signal(Option::<SecurityConfig>::None);
    let devices = create_rw_signal(Vec::<DeviceInfo>::new());
    let loading = create_rw_signal(true);
    let saving = create_rw_signal(false);
    let error = create_rw_signal(Option::<String>::None);

    // Load config and devices on mount
    create_effect(move |_| {
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

                loading.set(false);
            });
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
                <h1 class="text-2xl font-bold mb-6">"Security Configuration"</h1>

                {move || {
                    if loading.get() {
                        view! { <div class="text-gray-500">"Loading..."</div> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-red-50 dark:bg-red-900 text-red-700 dark:text-red-200 rounded">
                                        {e}
                                    </div>
                                })}

                                <GatewaySecuritySettings config=config />
                                <PairedDevices devices=devices state=state />

                                <div class="pt-4 border-t border-gray-200 dark:border-gray-700">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-6 py-2 bg-blue-500 text-white rounded hover:bg-blue-600 disabled:opacity-50"
                                    >
                                        {move || if saving.get() { "Saving..." } else { "Save Changes" }}
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
    view! {
        <div class="bg-white dark:bg-gray-800 p-6 rounded-lg border border-gray-200 dark:border-gray-700">
            <h2 class="text-lg font-semibold mb-4">"Gateway Security"</h2>

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
                    <label class="font-medium">"Require Authentication"</label>
                </div>
                <p class="text-sm text-gray-500 ml-6">
                    "Require clients to authenticate before connecting to the Gateway"
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
                    <label class="font-medium">"Enable Device Pairing"</label>
                </div>
                <p class="text-sm text-gray-500 ml-6">
                    "Allow new devices to pair with the Gateway using pairing codes"
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
                    <label class="font-medium">"Allow Guest Access"</label>
                </div>
                <p class="text-sm text-gray-500 ml-6">
                    "Allow temporary guest sessions without device pairing"
                </p>
            </div>
        </div>
    }
}

#[component]
fn PairedDevices(
    devices: RwSignal<Vec<DeviceInfo>>,
    state: DashboardState,
) -> impl IntoView {
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
        <div class="bg-white dark:bg-gray-800 p-6 rounded-lg border border-gray-200 dark:border-gray-700">
            <h2 class="text-lg font-semibold mb-4">"Paired Devices"</h2>

            <div class="space-y-3">
                {move || {
                    let device_list = devices.get();
                    if device_list.is_empty() {
                        view! {
                            <div class="text-gray-500 text-center py-4">
                                "No devices paired"
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
    let paired_date = device.paired_at.clone();
    let last_seen_text = device.last_seen.clone().unwrap_or_else(|| "Never".to_string());

    view! {
        <div class="flex items-center justify-between p-4 bg-gray-50 dark:bg-gray-700 rounded border border-gray-200 dark:border-gray-600">
            <div class="flex-1">
                <div class="font-medium">{device.device_name}</div>
                <div class="text-sm text-gray-500">
                    {device.device_type} " • " {device.device_id}
                </div>
                <div class="text-xs text-gray-400 mt-1">
                    "Paired: " {paired_date} " • Last seen: " {last_seen_text}
                </div>
            </div>
            <button
                on:click=move |_| on_revoke()
                class="px-3 py-1 bg-red-500 text-white text-sm rounded hover:bg-red-600"
            >
                "Revoke"
            </button>
        </div>
    }
}
