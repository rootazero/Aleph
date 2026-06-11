//! Security Configuration View
//!
//! Provides UI for managing security settings:
//! - Gateway security settings (require auth, enable pairing, allow guest)
//! - Paired devices management
//! - Device revocation
//! - Real-time updates via config events
//!
//! ## Layout
//! - [`gateway`] — `GatewaySecuritySettings`, `NetworkAccessSection`, `PairedDevices`, `DeviceCard`
//! - [`outbound`] — `OutboundSecuritySection`
//! - [`shell`] — `ShellSecuritySection`
//! - [`pii_rules`] — custom PII rule editor + section wrapper
//! - [`secrets`] — `SecretProtectionSection`
//! - [`pii_section`] — Search-side PII toggles
//! - [`sandbox`] — sandbox rate-limit + bucket cards
//! - this module — `SecurityView` entry point + shared `validate_regex` helper

mod gateway;
mod outbound;
mod pii_rules;
mod pii_section;
mod sandbox;
mod secrets;
mod shell;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{DeviceInfo, SearchConfig, SearchConfigApi, SecurityConfig, SecurityConfigApi};
use crate::context::DashboardState;
use crate::i18n::*;

use gateway::{GatewaySecuritySettings, NetworkAccessSection, PairedDevices};
use outbound::OutboundSecuritySection;
use pii_rules::CustomPiiRulesSection;
use pii_section::PIISection;
use sandbox::SandboxRateLimitSection;
use secrets::SecretProtectionSection;
use shell::ShellSecuritySection;

pub(super) fn validate_regex(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Ok(());
    }
    // Use js_sys::eval to test regex validity in JS context
    let escaped = pattern.replace('\'', "\\'");
    let js_code = format!(
        "try {{ new RegExp('{escaped}'); true; }} catch(e) {{ false; }}"
    );
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
    let needs_restart = RwSignal::new(false);

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
                        error.set(Some(format!("Failed to load security config: {e}")));
                    }
                }

                // Load devices
                match SecurityConfigApi::list_devices(&state).await {
                    Ok(devs) => {
                        devices.set(devs);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load devices: {e}")));
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
        let security_cfg = config.get();
        let search_cfg = search_config.get();
        spawn_local(async move {
            saving.set(true);
            needs_restart.set(false);

            // Order matters: the PII built-in toggles live under `[privacy]` and
            // SearchConfigApi::update rewrites that whole table via
            // `save_incremental(["privacy"])`. The custom PII rules also live
            // under `[privacy]` but are written by SecurityConfigApi::update as a
            // per-key `custom_rules` insert. We must run the whole-table rewrite
            // FIRST and the per-key custom_rules write LAST, otherwise the search
            // save clobbers the custom rules we just persisted.
            match SearchConfigApi::update(&state, search_cfg).await {
                Ok(_) => {}
                Err(e) => {
                    error.set(Some(format!("Failed to save PII config: {e}")));
                }
            }

            if let Some(cfg) = security_cfg {
                match SecurityConfigApi::update(&state, cfg).await {
                    Ok(restart) => {
                        error.set(None);
                        needs_restart.set(restart);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to save security config: {e}")));
                    }
                }
            }

            saving.set(false);
        });
    };

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto aleph-content-top">
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

                                {move || needs_restart.get().then(|| view! {
                                    <div class="p-3 bg-warning-subtle text-warning rounded">
                                        {t!(i18n, settings.security.restart_required)}
                                    </div>
                                })}

                                <GatewaySecuritySettings config=config />
                                <NetworkAccessSection config=config />
                                <OutboundSecuritySection config=config />
                                <ShellSecuritySection config=config />
                                <SecretProtectionSection config=config />
                                <SandboxRateLimitSection config=config />
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
