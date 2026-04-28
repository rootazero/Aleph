//! Security Configuration View
//!
//! Provides UI for managing security settings:
//! - Gateway security settings (require auth, enable pairing, allow guest)
//! - Paired devices management
//! - Device revocation
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{
    CustomLeakPattern, CustomPiiRule, CustomPiiSeverity, CustomRiskPattern, DeviceInfo,
    PiiAction, SearchConfig, SearchConfigApi, SecretsProtectionConfig, SecurityConfig,
    SecurityConfigApi, ShellSecurityConfig, VirtualKeyEntry,
};
use crate::context::DashboardState;
use crate::i18n::*;

fn validate_regex(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Ok(());
    }
    // Use js_sys::eval to test regex validity in JS context
    let escaped = pattern.replace('\'', "\\'");
    let js_code = format!("try {{ new RegExp('{}'); true; }} catch(e) {{ false; }}", escaped);
    match js_sys::eval(&js_code) {
        Ok(result) => match result.as_bool() {
            Some(true) => Ok(()),
            _ => Err("Invalid regex pattern".to_string()),
        },
        Err(_) => Err("Invalid regex pattern".to_string()),
    }
}

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
                                <OutboundSecuritySection config=config />
                                <ShellSecuritySection config=config />
                                <SecretProtectionSection config=config />
                                <CustomPiiRulesSection config=config />
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
fn GatewaySecuritySettings(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
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
fn NetworkAccessSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
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
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(
                            &serde_json::to_string(&result).unwrap_or_default(),
                        ) {
                            if v.get("needs_restart")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                            {
                                needs_restart.set(true);
                            }
                        }
                        needs_restart.set(true);
                        save_success.set(true);
                        set_timeout(
                            move || {
                                save_success.set(false);
                            },
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
fn PairedDevices(devices: RwSignal<Vec<DeviceInfo>>, state: DashboardState) -> impl IntoView {
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
fn DeviceCard<F>(device: DeviceInfo, on_revoke: F) -> impl IntoView
where
    F: Fn() + 'static,
{
    let i18n = use_i18n();
    let paired_date = device.paired_at.clone();
    let last_seen_text = device
        .last_seen
        .clone()
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
fn OutboundSecuritySection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                {t!(i18n, settings.security.outbound_protection)}
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.security.outbound_protection_desc)}
            </p>

            <div class="space-y-4">
                // Master toggle - SSRF enabled
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_enabled)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_enabled_desc)}</div>
                    </div>
                </label>

                // Tool LAN access toggle
                <label class="flex items-center space-x-3 cursor-pointer ml-4">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.ssrf_allow_tool_private_network).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allow_tool_private_network = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_allow_tool_lan)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_allow_tool_lan_desc)}</div>
                    </div>
                </label>

                // Webhook LAN access toggle
                <label class="flex items-center space-x-3 cursor-pointer ml-4">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.ssrf_allow_webhook_private_network).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allow_webhook_private_network = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_allow_webhook_lan)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_allow_webhook_lan_desc)}</div>
                    </div>
                </label>

                // Max redirects number input
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_max_redirects)}
                    </label>
                    <input
                        type="number"
                        min="0"
                        max="20"
                        prop:value=move || config.get().map(|c| c.ssrf_max_redirects.to_string()).unwrap_or_else(|| "5".to_string())
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                                    cfg.ssrf_max_redirects = v.min(20);
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-24 px-3 py-1 bg-surface-sunken border border-border rounded text-text-primary disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_max_redirects_desc)}</p>
                </div>

                // Allowed hosts textarea
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_allowed_hosts)}
                    </label>
                    <textarea
                        prop:value=move || config.get().map(|c| c.ssrf_allowed_hosts.join("\n")).unwrap_or_default()
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allowed_hosts = event_target_value(&ev)
                                    .lines()
                                    .map(|l| l.trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect();
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        placeholder=move || t_string!(i18n, settings.security.ssrf_allowed_hosts_placeholder).to_string()
                        rows="3"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_allowed_hosts_desc)}</p>
                </div>

                // Blocked hosts textarea
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_blocked_hosts)}
                    </label>
                    <textarea
                        prop:value=move || config.get().map(|c| c.ssrf_blocked_hosts.join("\n")).unwrap_or_default()
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_blocked_hosts = event_target_value(&ev)
                                    .lines()
                                    .map(|l| l.trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect();
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        placeholder=move || t_string!(i18n, settings.security.ssrf_blocked_hosts_placeholder).to_string()
                        rows="3"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_blocked_hosts_desc)}</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn ShellSecuritySection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let pattern_errors = RwSignal::new(Vec::<(usize, String, String)>::new());

    let validate_all_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg.shell_security.custom_blocked.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "blocked".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_danger.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "danger".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_safe.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "safe".to_string(), e));
                }
            }
        }
        let is_valid = errors.is_empty();
        pattern_errors.set(errors);
        is_valid
    };

    let add_pattern = move |category: &'static str| {
        if let Some(mut cfg) = config.get() {
            let new_pattern = CustomRiskPattern {
                pattern: String::new(),
                reason: None,
            };
            match category {
                "blocked" => cfg.shell_security.custom_blocked.push(new_pattern),
                "danger" => cfg.shell_security.custom_danger.push(new_pattern),
                "safe" => cfg.shell_security.custom_safe.push(new_pattern),
                _ => {}
            }
            config.set(Some(cfg));
        }
    };

    let remove_pattern = move |category: &'static str, index: usize| {
        if let Some(mut cfg) = config.get() {
            match category {
                "blocked" => {
                    cfg.shell_security.custom_blocked.remove(index);
                }
                "danger" => {
                    cfg.shell_security.custom_danger.remove(index);
                }
                "safe" => {
                    cfg.shell_security.custom_safe.remove(index);
                }
                _ => {}
            }
            config.set(Some(cfg));
            validate_all_patterns();
        }
    };

    let update_pattern =
        move |category: &'static str, index: usize, field: &'static str, value: String| {
            if let Some(mut cfg) = config.get() {
                let pattern = match category {
                    "blocked" => cfg.shell_security.custom_blocked.get_mut(index),
                    "danger" => cfg.shell_security.custom_danger.get_mut(index),
                    "safe" => cfg.shell_security.custom_safe.get_mut(index),
                    _ => None,
                };
                if let Some(p) = pattern {
                    match field {
                        "pattern" => p.pattern = value,
                        "reason" => p.reason = if value.is_empty() {
                            None
                        } else {
                            Some(value)
                        },
                        _ => {}
                    }
                }
                config.set(Some(cfg));
                validate_all_patterns();
            }
        };

    let has_error = move |category: &'static str, index: usize| -> bool {
        pattern_errors
            .get()
            .iter()
            .any(|(i, c, _)| *i == index && c == category)
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Shell Command Security"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure custom risk patterns for shell command execution."
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || {
                            config.get()
                                .map(|c| c.shell_security.enable_custom_patterns)
                                .unwrap_or(false)
                        }
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.shell_security.enable_custom_patterns =
                                    event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">
                            "Enable Custom Risk Patterns"
                        </div>
                        <div class="text-xs text-text-tertiary">
                            "When enabled, custom patterns supplement built-in security rules"
                        </div>
                    </div>
                </label>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Blocked Patterns (execution denied)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_blocked.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("blocked", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "blocked",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "blocked",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("blocked", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("blocked")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Blocked Pattern"
                    </button>
                </div>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Danger Patterns (require approval)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_danger.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("danger", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "danger",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "danger",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("danger", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("danger")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Danger Pattern"
                    </button>
                </div>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Safe Patterns (auto-approved)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_safe.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("safe", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "safe",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "safe",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("safe", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("safe")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Safe Pattern"
                    </button>
                </div>

                {move || {
                    let errors = pattern_errors.get();
                    if !errors.is_empty() {
                        Some(view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                <ul class="list-disc list-inside">
                                    {errors.iter().map(|(i, cat, err)| view! {
                                        <li>{format!("{} #{}: {}", cat, i + 1, err)}</li>
                                    }).collect::<Vec<_>>()}
                                </ul>
                            </div>
                        })
                    } else {
                        None
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn CustomPiiRulesSubsection(
    rules: RwSignal<Vec<CustomPiiRule>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>,
) -> impl IntoView {
    let validate_all = move || {
        let mut errors = Vec::new();
        for (i, rule) in rules.get().iter().enumerate() {
            if let Err(e) = validate_regex(&rule.pattern) {
                errors.push((i, e));
            }
        }
        let is_valid = errors.is_empty();
        pattern_errors.set(errors);
        is_valid
    };

    let add_rule = move || {
        let mut current = rules.get();
        current.push(CustomPiiRule {
            name: String::new(),
            pattern: String::new(),
            placeholder: "[CUSTOM_PII]".to_string(),
            severity: CustomPiiSeverity::Medium,
            action: PiiAction::Block,
        });
        rules.set(current);
    };

    let remove_rule = move |index: usize| {
        let mut current = rules.get();
        current.remove(index);
        rules.set(current);
        validate_all();
    };

    let update_rule = move |index: usize, field: &'static str, value: String| {
        let mut current = rules.get();
        if let Some(rule) = current.get_mut(index) {
            match field {
                "name" => rule.name = value,
                "pattern" => rule.pattern = value,
                "placeholder" => rule.placeholder = value,
                "severity" => rule.severity = match value.as_str() {
                    "low" => CustomPiiSeverity::Low,
                    "medium" => CustomPiiSeverity::Medium,
                    "high" => CustomPiiSeverity::High,
                    "critical" => CustomPiiSeverity::Critical,
                    _ => CustomPiiSeverity::Medium,
                },
                "action" => rule.action = match value.as_str() {
                    "block" => PiiAction::Block,
                    "warn" => PiiAction::Warn,
                    "off" => PiiAction::Off,
                    _ => PiiAction::Block,
                },
                _ => {}
            }
        }
        rules.set(current);
        validate_all();
    };

    view! {
        <div class="mt-6 pt-6 border-t border-border">
            <h3 class="text-sm font-semibold text-text-secondary mb-3">
                "Custom PII Rules"
            </h3>

            <div class="space-y-3">
                {move || {
                    let rule_list = rules.get();
                    rule_list.into_iter().enumerate().map(|(i, rule)| {
                        let has_err = pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                        view! {
                            <div class="p-3 bg-surface-sunken rounded border border-border space-y-2">
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.name.clone()
                                        on:input=move |ev| update_rule(i, "name", event_target_value(&ev))
                                        placeholder="Rule name..."
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <button
                                        on:click=move |_| remove_rule(i)
                                        class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                    >
                                        "Remove"
                                    </button>
                                </div>
                                <input
                                    type="text"
                                    prop:value=rule.pattern.clone()
                                    on:input=move |ev| update_rule(i, "pattern", event_target_value(&ev))
                                    placeholder="Regex pattern..."
                                    class=move || format!("w-full px-3 py-1 bg-surface-raised border rounded text-sm text-text-primary {}",
                                        if has_err { "border-danger" } else { "border-border" })
                                />
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.placeholder.clone()
                                        on:input=move |ev| update_rule(i, "placeholder", event_target_value(&ev))
                                        placeholder="Placeholder..."
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <select
                                        prop:value=move || match rule.severity {
                                            CustomPiiSeverity::Low => "low",
                                            CustomPiiSeverity::Medium => "medium",
                                            CustomPiiSeverity::High => "high",
                                            CustomPiiSeverity::Critical => "critical",
                                        }
                                        on:change=move |ev| update_rule(i, "severity", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="low">"Low"</option>
                                        <option value="medium">"Medium"</option>
                                        <option value="high">"High"</option>
                                        <option value="critical">"Critical"</option>
                                    </select>
                                    <select
                                        prop:value=move || match rule.action {
                                            PiiAction::Block => "block",
                                            PiiAction::Warn => "warn",
                                            PiiAction::Off => "off",
                                        }
                                        on:change=move |ev| update_rule(i, "action", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="block">"Block"</option>
                                        <option value="warn">"Warn"</option>
                                        <option value="off">"Off"</option>
                                    </select>
                                </div>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>

            <button
                on:click=move |_| add_rule()
                class="mt-3 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
            >
                "+ Add Custom Rule"
            </button>

            {move || {
                let errors = pattern_errors.get();
                if !errors.is_empty() {
                    Some(view! {
                        <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                            <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                            <ul class="list-disc list-inside">
                                {errors.iter().map(|(i, err)| view! {
                                    <li>{format!("Rule #{}: {}", i + 1, err)}</li>
                                }).collect::<Vec<_>>()}
                            </ul>
                        </div>
                    })
                } else {
                    None
                }
            }}
        </div>
    }
}

#[component]
fn CustomPiiRulesSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let custom_rules = RwSignal::new(
        config.get().map(|c| c.custom_pii_rules.clone()).unwrap_or_default(),
    );
    let pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

    let save = move |_| {
        if let Some(mut cfg) = config.get() {
            cfg.custom_pii_rules = custom_rules.get();
            config.set(Some(cfg));
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex justify-between items-start mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary">"Custom PII Rules"</h2>
                    <p class="text-sm text-text-tertiary mt-1">
                        "Define custom PII patterns with severity and action settings."
                    </p>
                </div>
                <button
                    on:click=save
                    class="px-4 py-1.5 text-sm bg-info text-white rounded hover:bg-primary-hover"
                >
                    "Apply"
                </button>
            </div>
            <CustomPiiRulesSubsection rules=custom_rules pattern_errors=pattern_errors />
        </div>
    }
}

#[component]
fn SecretProtectionSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let leak_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

    let validate_leak_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg.secrets_protection.custom_leak_patterns.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, e));
                }
            }
        }
        let is_valid = errors.is_empty();
        leak_pattern_errors.set(errors);
        is_valid
    };

    let add_virtual_key = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.push(VirtualKeyEntry {
                alias: String::new(),
                secret_name: String::new(),
            });
            config.set(Some(cfg));
        }
    };

    let remove_virtual_key = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.remove(index);
            config.set(Some(cfg));
        }
    };

    let update_virtual_key = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(entry) = cfg.secrets_protection.virtual_keys.get_mut(index) {
                match field {
                    "alias" => entry.alias = value,
                    "secret_name" => entry.secret_name = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
        }
    };

    let add_leak_pattern = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.custom_leak_patterns.push(CustomLeakPattern {
                name: String::new(),
                pattern: String::new(),
            });
            config.set(Some(cfg));
        }
    };

    let remove_leak_pattern = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.custom_leak_patterns.remove(index);
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    let update_leak_pattern = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(pattern) = cfg.secrets_protection.custom_leak_patterns.get_mut(index) {
                match field {
                    "name" => pattern.name = value,
                    "pattern" => pattern.pattern = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Secret Protection"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure virtual key aliases and custom leak detection patterns."
            </p>

            <div class="space-y-6">
                <div>
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Virtual Key Aliases"</h3>
                    <div class="space-y-2">
                        {move || {
                            let keys = config.get().map(|c| c.secrets_protection.virtual_keys.clone()).unwrap_or_default();
                            keys.into_iter().enumerate().map(|(i, entry)| {
                                view! {
                                    <div class="flex gap-2 items-center">
                                        <input
                                            type="text"
                                            prop:value=entry.alias.clone()
                                            on:input=move |ev| update_virtual_key(i, "alias", event_target_value(&ev))
                                            placeholder="Alias (e.g., openai)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <span class="text-text-tertiary">"→"</span>
                                        <input
                                            type="text"
                                            prop:value=entry.secret_name.clone()
                                            on:input=move |ev| update_virtual_key(i, "secret_name", event_target_value(&ev))
                                            placeholder="Secret name (e.g., OPENAI_API_KEY)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <button
                                            on:click=move |_| remove_virtual_key(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_virtual_key()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Virtual Key"
                    </button>
                </div>

                <div class="pt-4 border-t border-border">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Custom Leak Detection Patterns"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.secrets_protection.custom_leak_patterns.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = leak_pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.name.clone()
                                                on:input=move |ev| update_leak_pattern(i, "name", event_target_value(&ev))
                                                placeholder="Pattern name..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_leak_pattern(i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_leak_pattern(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_leak_pattern()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Leak Pattern"
                    </button>

                    {move || {
                        let errors = leak_pattern_errors.get();
                        if !errors.is_empty() {
                            Some(view! {
                                <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                    <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                    <ul class="list-disc list-inside">
                                        {errors.iter().map(|(i, err)| view! {
                                            <li>{format!("Pattern #{}: {}", i + 1, err)}</li>
                                        }).collect::<Vec<_>>()}
                                    </ul>
                                </div>
                            })
                        } else {
                            None
                        }
                    }}
                </div>
            </div>
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
