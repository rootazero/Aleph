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

#[component]
pub fn SecurityView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = create_rw_signal(Option::<SecurityConfig>::None);
    let devices = create_rw_signal(Vec::<DeviceInfo>::new());
    let search_config = RwSignal::new(SearchConfig {
        enabled: false,
        default_provider: "tavily".to_string(),
        max_results: 5,
        timeout_seconds: 10,
        pii_enabled: false,
        pii_scrub_email: true,
        pii_scrub_phone: true,
        pii_scrub_ssn: true,
        pii_scrub_credit_card: true,
    });
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
                <h1 class="text-2xl font-bold mb-6">"Security Configuration"</h1>

                {move || {
                    if loading.get() {
                        view! { <div class="text-text-tertiary">"Loading..."</div> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle text-danger rounded">
                                        {e}
                                    </div>
                                })}

                                <GatewaySecuritySettings config=config />
                                <PIISection config=search_config />
                                <PairedDevices devices=devices state=state />

                                <div class="pt-4 border-t border-border">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-6 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
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
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
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
                <p class="text-sm text-text-tertiary ml-6">
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
                <p class="text-sm text-text-tertiary ml-6">
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
                <p class="text-sm text-text-tertiary ml-6">
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
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">"Paired Devices"</h2>

            <div class="space-y-3">
                {move || {
                    let device_list = devices.get();
                    if device_list.is_empty() {
                        view! {
                            <div class="text-text-tertiary text-center py-4">
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
        <div class="flex items-center justify-between p-4 bg-surface-sunken rounded border border-border">
            <div class="flex-1">
                <div class="font-medium">{device.device_name}</div>
                <div class="text-sm text-text-tertiary">
                    {device.device_type} " • " {device.device_id}
                </div>
                <div class="text-xs text-text-secondary mt-1">
                    "Paired: " {paired_date} " • Last seen: " {last_seen_text}
                </div>
            </div>
            <button
                on:click=move |_| on_revoke()
                class="px-3 py-1 bg-danger text-white text-sm rounded hover:bg-danger"
            >
                "Revoke"
            </button>
        </div>
    }
}

#[component]
fn PIISection(config: RwSignal<SearchConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let pii_enabled = RwSignal::new(config.get().pii_enabled);
    let scrub_email = RwSignal::new(config.get().pii_scrub_email);
    let scrub_phone = RwSignal::new(config.get().pii_scrub_phone);
    let scrub_ssn = RwSignal::new(config.get().pii_scrub_ssn);
    let scrub_credit_card = RwSignal::new(config.get().pii_scrub_credit_card);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
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
            <h2 class="text-lg font-semibold text-text-primary mb-4">"PII Scrubbing"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Automatically remove personally identifiable information from search results"
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || pii_enabled.get()
                        on:change=move |ev| pii_enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">"Enable PII Scrubbing"</div>
                        <div class="text-sm text-text-tertiary">"Remove sensitive information from results"</div>
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
                        <span class="text-sm text-text-secondary">"Email addresses"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_phone.get()
                            on:change=move |ev| scrub_phone.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">"Phone numbers"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_ssn.get()
                            on:change=move |ev| scrub_ssn.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">"Social Security Numbers"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_credit_card.get()
                            on:change=move |ev| scrub_credit_card.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-text-secondary">"Credit card numbers"</span>
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
                                "Saved successfully"
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
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>
        </div>
    }
}
