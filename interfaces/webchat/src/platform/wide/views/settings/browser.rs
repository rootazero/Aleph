//! Browser Settings View
//!
//! Provides UI for managing browser configuration: driver mode, engine,
//! headless, `DevTools` profile, and security settings.

use crate::api::{BrowserConfig, BrowserConfigApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::settings::browser_runtime_banner::RuntimeSummaryBanner;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Macro for per-section save logic (avoids Box<dyn Fn()> Send+Sync issues)
// ============================================================================

macro_rules! section_save {
    ($config:expr) => {{
        let state = expect_context::<DashboardState>();
        let saving = RwSignal::new(false);
        let save_error = RwSignal::new(Option::<String>::None);
        let save_success = RwSignal::new(false);
        let config = $config;
        let save_fn = StoredValue::new(move || {
            saving.set(true);
            save_error.set(None);
            save_success.set(false);
            let cfg = config.get();
            spawn_local(async move {
                match BrowserConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        saving.set(false);
                        save_success.set(true);
                        set_timeout(
                            move || save_success.set(false),
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
        (saving, save_error, save_success, save_fn)
    }};
}

// ============================================================================
// Main view
// ============================================================================

#[component]
#[must_use]
pub fn BrowserView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(BrowserConfig {
        default_driver: "managed".to_string(),
        headless: true,
        devtools_profile: "user".to_string(),
        block_private: true,
        blocked_domains: Vec::new(),
        allowed_domains: Vec::new(),
        nav_timeout_secs: 30,
        action_timeout_secs: 10,
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    let load_config = move || {
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            match BrowserConfigApi::get(&state).await {
                Ok(cfg) => {
                    config.set(cfg);
                    error.set(None);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load config: {e}")));
                    loading.set(false);
                }
            }
        });
    };

    load_config();

    view! {
        <div class="px-6 pb-6 aleph-content-top space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, browser_settings.title)}</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    {t!(i18n, browser_settings.description)}
                </p>
            </div>

            <RuntimeSummaryBanner />

            <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm">
                {t!(i18n, browser_settings.restart_hint)}
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">{t!(i18n, browser_settings.loading)}</div>
                        </div>
                    }.into_any()
                } else if error.get().is_some() {
                    view! {
                        <div class="space-y-6">
                            {move || {
                                match error.get() {
                                    Some(e) if e.contains("Send failed") || e.contains("Failed to load") => {
                                        Some(view! {
                                            <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2">
                                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                    <circle cx="12" cy="12" r="10"/>
                                                    <line x1="12" y1="16" x2="12" y2="12"/>
                                                    <line x1="12" y1="8" x2="12.01" y2="8"/>
                                                </svg>
                                                {t!(i18n, browser_settings.gateway_unavailable)}
                                            </div>
                                        }.into_any())
                                    }
                                    Some(e) => {
                                        Some(view! {
                                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                                {e}
                                            </div>
                                        }.into_any())
                                    }
                                    None => None,
                                }
                            }}
                            <button
                                on:click=move |_| load_config()
                                class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover"
                            >
                                "Retry"
                            </button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-6">
                            <DefaultModeSection config=config />
                            <EngineSection config=config />
                            <DevToolsSection config=config />
                            <SecuritySection config=config />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// Section: Default Browser Mode
// ============================================================================

#[component]
fn DefaultModeSection(config: RwSignal<BrowserConfig>) -> impl IntoView {
    let i18n = use_i18n();
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, browser_settings.default_mode_title)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, browser_settings.default_mode_description)}
            </p>

            <div class="space-y-3">
                <label class=move || {
                    if config.get().default_driver == "managed" {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-primary bg-primary/5 transition-colors"
                    } else {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-border hover:bg-surface-hover transition-colors"
                    }
                }>
                    <input
                        type="radio"
                        name="default_driver"
                        value="managed"
                        checked=move || config.get().default_driver == "managed"
                        on:change=move |_| {
                            config.update(|c| c.default_driver = "managed".to_string());
                            save_fn.with_value(|f| f());
                        }
                        class="mt-1 w-4 h-4 text-primary focus:ring-primary/30"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.mode_playwright_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.mode_playwright_desc)}</div>
                    </div>
                </label>

                <label class=move || {
                    if config.get().default_driver == "existing_session" {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-primary bg-primary/5 transition-colors"
                    } else {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-border hover:bg-surface-hover transition-colors"
                    }
                }>
                    <input
                        type="radio"
                        name="default_driver"
                        value="existing_session"
                        checked=move || config.get().default_driver == "existing_session"
                        on:change=move |_| {
                            config.update(|c| c.default_driver = "existing_session".to_string());
                            save_fn.with_value(|f| f());
                        }
                        class="mt-1 w-4 h-4 text-primary focus:ring-primary/30"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.mode_devtools_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.mode_devtools_desc)}</div>
                    </div>
                </label>

                <SaveFeedback saving=saving save_error=save_error save_success=save_success />
            </div>
        </div>
    }
}

// ============================================================================
// Section: Browser Engine & Headless
// ============================================================================

#[component]
fn EngineSection(config: RwSignal<BrowserConfig>) -> impl IntoView {
    let i18n = use_i18n();
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, browser_settings.engine_title)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, browser_settings.engine_description)}
            </p>

            <div class="space-y-5">
                // Headless toggle
                <div class="flex items-center justify-between">
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.headless_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.headless_desc)}</div>
                    </div>
                    <button
                        on:click=move |_| {
                            config.update(|c| c.headless = !c.headless);
                            save_fn.with_value(|f| f());
                        }
                        class=move || {
                            if config.get().headless {
                                "relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent bg-primary transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-primary/30"
                            } else {
                                "relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent bg-surface-sunken transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-primary/30"
                            }
                        }
                    >
                        <span
                            class=move || {
                                if config.get().headless {
                                    "pointer-events-none inline-block h-5 w-5 translate-x-5 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out"
                                } else {
                                    "pointer-events-none inline-block h-5 w-5 translate-x-0 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out"
                                }
                            }
                        />
                    </button>
                </div>

                // Navigation timeout
                <div>
                    <label class="block text-sm font-medium text-text-primary mb-2">{t!(i18n, browser_settings.nav_timeout_label)}</label>
                    <input
                        type="number"
                        min="5" max="300"
                        prop:value=move || config.get().nav_timeout_secs as i64
                        on:change=move |ev| {
                            let val = event_target_value(&ev).parse::<u64>().unwrap_or(30);
                            config.update(|c| c.nav_timeout_secs = val);
                            save_fn.with_value(|f| f());
                        }
                        class="block w-32 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                    />
                </div>

                // Action timeout
                <div>
                    <label class="block text-sm font-medium text-text-primary mb-2">{t!(i18n, browser_settings.action_timeout_label)}</label>
                    <input
                        type="number"
                        min="1" max="60"
                        prop:value=move || config.get().action_timeout_secs as i64
                        on:change=move |ev| {
                            let val = event_target_value(&ev).parse::<u64>().unwrap_or(10);
                            config.update(|c| c.action_timeout_secs = val);
                            save_fn.with_value(|f| f());
                        }
                        class="block w-32 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                    />
                </div>

                <SaveFeedback saving=saving save_error=save_error save_success=save_success />
            </div>
        </div>
    }
}

// ============================================================================
// Section: Chrome DevTools Settings
// ============================================================================

#[component]
fn DevToolsSection(config: RwSignal<BrowserConfig>) -> impl IntoView {
    let i18n = use_i18n();
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, browser_settings.devtools_title)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, browser_settings.devtools_description)}
            </p>

            <div class="space-y-3">
                <label class=move || {
                    if config.get().devtools_profile == "user" {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-primary bg-primary/5 transition-colors"
                    } else {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-border hover:bg-surface-hover transition-colors"
                    }
                }>
                    <input
                        type="radio"
                        name="devtools_profile"
                        value="user"
                        checked=move || config.get().devtools_profile == "user"
                        on:change=move |_| {
                            config.update(|c| c.devtools_profile = "user".to_string());
                            save_fn.with_value(|f| f());
                        }
                        class="mt-1 w-4 h-4 text-primary focus:ring-primary/30"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.profile_user_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.profile_user_desc)}</div>
                    </div>
                </label>

                <label class=move || {
                    if config.get().devtools_profile == "managed" {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-primary bg-primary/5 transition-colors"
                    } else {
                        "flex items-start space-x-3 cursor-pointer p-3 rounded-lg border border-border hover:bg-surface-hover transition-colors"
                    }
                }>
                    <input
                        type="radio"
                        name="devtools_profile"
                        value="managed"
                        checked=move || config.get().devtools_profile == "managed"
                        on:change=move |_| {
                            config.update(|c| c.devtools_profile = "managed".to_string());
                            save_fn.with_value(|f| f());
                        }
                        class="mt-1 w-4 h-4 text-primary focus:ring-primary/30"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.profile_managed_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.profile_managed_desc)}</div>
                    </div>
                </label>

                <SaveFeedback saving=saving save_error=save_error save_success=save_success />
            </div>
        </div>
    }
}

// ============================================================================
// Section: Security
// ============================================================================

#[component]
fn SecuritySection(config: RwSignal<BrowserConfig>) -> impl IntoView {
    let i18n = use_i18n();
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, browser_settings.security_title)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, browser_settings.security_description)}
            </p>

            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, browser_settings.block_private_label)}</div>
                        <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.block_private_desc)}</div>
                    </div>
                    <button
                        on:click=move |_| {
                            config.update(|c| c.block_private = !c.block_private);
                            save_fn.with_value(|f| f());
                        }
                        class=move || {
                            if config.get().block_private {
                                "relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent bg-primary transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-primary/30"
                            } else {
                                "relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent bg-surface-sunken transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-primary/30"
                            }
                        }
                    >
                        <span
                            class=move || {
                                if config.get().block_private {
                                    "pointer-events-none inline-block h-5 w-5 translate-x-5 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out"
                                } else {
                                    "pointer-events-none inline-block h-5 w-5 translate-x-0 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out"
                                }
                            }
                        />
                    </button>
                </div>

                // Blocked domains (one per line)
                <div>
                    <label class="block text-sm font-medium text-text-primary mb-1">{t!(i18n, browser_settings.blocked_domains_label)}</label>
                    <p class="text-xs text-text-tertiary mb-2">{t!(i18n, browser_settings.blocked_domains_desc)}</p>
                    <textarea
                        rows="3"
                        prop:value=move || config.get().blocked_domains.join("\n")
                        on:change=move |ev| {
                            let list = parse_domain_lines(&event_target_value(&ev));
                            config.update(|c| c.blocked_domains = list);
                            save_fn.with_value(|f| f());
                        }
                        placeholder=move || t_string!(i18n, browser_settings.domains_placeholder).to_string()
                        class="block w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm font-mono text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                    />
                </div>

                // Allowed domains (allowlist mode — when non-empty, only these are reachable)
                <div>
                    <label class="block text-sm font-medium text-text-primary mb-1">{t!(i18n, browser_settings.allowed_domains_label)}</label>
                    <p class="text-xs text-text-tertiary mb-2">{t!(i18n, browser_settings.allowed_domains_desc)}</p>
                    <textarea
                        rows="3"
                        prop:value=move || config.get().allowed_domains.join("\n")
                        on:change=move |ev| {
                            let list = parse_domain_lines(&event_target_value(&ev));
                            config.update(|c| c.allowed_domains = list);
                            save_fn.with_value(|f| f());
                        }
                        placeholder=move || t_string!(i18n, browser_settings.domains_placeholder).to_string()
                        class="block w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm font-mono text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                    />
                </div>

                <SaveFeedback saving=saving save_error=save_error save_success=save_success />
            </div>
        </div>
    }
}

/// Split a textarea value into a trimmed, non-empty domain list (one per line).
fn parse_domain_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

// ============================================================================
// Shared save feedback component
// ============================================================================

#[component]
fn SaveFeedback(
    saving: RwSignal<bool>,
    save_error: RwSignal<Option<String>>,
    save_success: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || {
            if saving.get() {
                Some(view! {
                    <div class="text-sm text-text-tertiary">{t!(i18n, browser_settings.saving)}</div>
                }.into_any())
            } else if let Some(e) = save_error.get() {
                Some(view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                }.into_any())
            } else if save_success.get() {
                Some(view! {
                    <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                        {t!(i18n, browser_settings.saved)}
                    </div>
                }.into_any())
            } else {
                None
            }
        }}
    }
}
