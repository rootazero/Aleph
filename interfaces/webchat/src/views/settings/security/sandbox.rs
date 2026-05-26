//! Sandbox rate-limit + per-tool bucket cards.

use leptos::prelude::*;

use crate::api::{SecurityConfig, WindowConfigSchema};
use crate::i18n::*;

#[component]
pub(super) fn SandboxRateLimitSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let get_rl = move || {
        config
            .get()
            .map(|c| c.sandbox_rate_limit.clone())
            .unwrap_or_default()
    };

    let set_enabled = move |v: bool| {
        if let Some(mut cfg) = config.get() {
            cfg.sandbox_rate_limit.enabled = v;
            config.set(Some(cfg));
        }
    };

    let set_exempt_loopback = move |v: bool| {
        if let Some(mut cfg) = config.get() {
            cfg.sandbox_rate_limit.exempt_loopback = v;
            config.set(Some(cfg));
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary">"Sandbox Rate Limiting"</h2>
                    <p class="text-sm text-text-tertiary mt-1">
                        "Limit tool execution frequency per session and category."
                    </p>
                </div>
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || get_rl().enabled
                        on:change=move |ev| set_enabled(event_target_checked(&ev))
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <span class="text-sm font-medium text-text-primary">"Enabled"</span>
                </label>
            </div>

            <div class="space-y-4 ml-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || get_rl().exempt_loopback
                        on:change=move |ev| set_exempt_loopback(event_target_checked(&ev))
                        disabled=move || !get_rl().enabled
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                    />
                    <div>
                        <div class="text-sm font-medium text-text-primary">"Exempt loopback"</div>
                        <div class="text-xs text-text-tertiary">"Skip rate limiting for localhost/tool requests"</div>
                    </div>
                </label>

                <div class="grid grid-cols-2 gap-4">
                    <RateLimitBucketCard
                        config=config
                        category="read"
                        label="Read Tools"
                        desc="Files, search, memory operations"
                    />
                    <RateLimitBucketCard
                        config=config
                        category="write"
                        label="Write Tools"
                        desc="Code generation, file writes, git operations"
                    />
                    <RateLimitBucketCard
                        config=config
                        category="dangerous"
                        label="Dangerous Tools"
                        desc="Shell execution, system commands"
                    />
                    <RateLimitBucketCard
                        config=config
                        category="admin"
                        label="Admin Tools"
                        desc="Gateway config, device management"
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
pub(super) fn RateLimitBucketCard(
    config: RwSignal<Option<SecurityConfig>>,
    category: &'static str,
    label: &'static str,
    desc: &'static str,
) -> impl IntoView {
    let get_window = move || {
        config
            .get()
            .map(|c| match category {
                "read" => c.sandbox_rate_limit.read.clone(),
                "write" => c.sandbox_rate_limit.write.clone(),
                "dangerous" => c.sandbox_rate_limit.dangerous.clone(),
                "admin" => c.sandbox_rate_limit.admin.clone(),
                _ => WindowConfigSchema::default(),
            })
            .unwrap_or_default()
    };

    let is_enabled = move || {
        config
            .get()
            .map(|c| c.sandbox_rate_limit.enabled)
            .unwrap_or(false)
    };

    view! {
        <div class="p-3 bg-surface-sunken rounded border border-border">
            <div class="mb-2">
                <div class="text-sm font-semibold text-text-primary">{label}</div>
                <div class="text-xs text-text-tertiary">{desc}</div>
            </div>
            <div class="grid grid-cols-3 gap-2 text-xs">
                <div>
                    <label class="text-text-tertiary">"Max/min"</label>
                    <input
                        type="number"
                        min="1"
                        max="1000"
                        prop:value=get_window().max_requests.to_string()
                        on:change=move |ev| {
                            if let (Some(mut cfg), Ok(v)) = (config.get(), event_target_value(&ev).parse::<u32>()) {
                                match category {
                                    "read" => cfg.sandbox_rate_limit.read.max_requests = v,
                                    "write" => cfg.sandbox_rate_limit.write.max_requests = v,
                                    "dangerous" => cfg.sandbox_rate_limit.dangerous.max_requests = v,
                                    "admin" => cfg.sandbox_rate_limit.admin.max_requests = v,
                                    _ => {}
                                }
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !is_enabled()
                        class="w-full px-2 py-1 bg-surface-raised border border-border rounded text-text-primary disabled:opacity-50"
                    />
                </div>
                <div>
                    <label class="text-text-tertiary">"Window(s)"</label>
                    <input
                        type="number"
                        min="1"
                        max="3600"
                        prop:value=get_window().window_secs.to_string()
                        on:change=move |ev| {
                            if let (Some(mut cfg), Ok(v)) = (config.get(), event_target_value(&ev).parse::<u64>()) {
                                match category {
                                    "read" => cfg.sandbox_rate_limit.read.window_secs = v,
                                    "write" => cfg.sandbox_rate_limit.write.window_secs = v,
                                    "dangerous" => cfg.sandbox_rate_limit.dangerous.window_secs = v,
                                    "admin" => cfg.sandbox_rate_limit.admin.window_secs = v,
                                    _ => {}
                                }
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !is_enabled()
                        class="w-full px-2 py-1 bg-surface-raised border border-border rounded text-text-primary disabled:opacity-50"
                    />
                </div>
                <div>
                    <label class="text-text-tertiary">"Burst"</label>
                    <input
                        type="number"
                        min="0"
                        max="100"
                        prop:value=get_window().burst_allow.to_string()
                        on:change=move |ev| {
                            if let (Some(mut cfg), Ok(v)) = (config.get(), event_target_value(&ev).parse::<u32>()) {
                                match category {
                                    "read" => cfg.sandbox_rate_limit.read.burst_allow = v,
                                    "write" => cfg.sandbox_rate_limit.write.burst_allow = v,
                                    "dangerous" => cfg.sandbox_rate_limit.dangerous.burst_allow = v,
                                    "admin" => cfg.sandbox_rate_limit.admin.burst_allow = v,
                                    _ => {}
                                }
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !is_enabled()
                        class="w-full px-2 py-1 bg-surface-raised border border-border rounded text-text-primary disabled:opacity-50"
                    />
                </div>
            </div>
        </div>
    }
}
