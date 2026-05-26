//! Outbound security section.

use leptos::prelude::*;

use crate::api::SecurityConfig;
use crate::i18n::*;

#[component]
pub(super) fn OutboundSecuritySection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
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

