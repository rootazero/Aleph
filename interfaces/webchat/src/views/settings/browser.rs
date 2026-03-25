//! Browser Settings View
//!
//! Provides UI for managing browser configuration: driver mode, engine,
//! headless, DevTools profile, and security settings.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{BrowserConfig, BrowserConfigApi};

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
                        set_timeout(move || save_success.set(false), std::time::Duration::from_secs(2));
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
pub fn BrowserView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(BrowserConfig {
        default_driver: "managed".to_string(),
        browser_engine: "chromium".to_string(),
        headless: true,
        devtools_profile: "user".to_string(),
        block_private: true,
        blocked_domains: Vec::new(),
        allowed_domains: Vec::new(),
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    spawn_local(async move {
        match BrowserConfigApi::get(&state).await {
            Ok(cfg) => {
                config.set(cfg);
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load config: {}", e)));
                loading.set(false);
            }
        }
    });

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Browser"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Configure browser automation for web browsing tools."
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">"Loading..."</div>
                        </div>
                    }.into_any()
                } else {
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
                                                "Gateway unavailable. Settings will load when connected."
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
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Default Browser Mode"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Choose the default browser backend when AI tools need web access."
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
                        <div class="font-medium text-text-primary">"Playwright (Headless)"</div>
                        <div class="text-sm text-text-tertiary">"Fast, invisible managed browser. Best for automated tasks like web scraping and screenshots."</div>
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
                        <div class="font-medium text-text-primary">"Chrome DevTools (Visible)"</div>
                        <div class="text-sm text-text-tertiary">"Connects to your running Chrome via DevTools Protocol. Access logged-in sessions and cookies."</div>
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
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Playwright Settings"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure the managed browser used in Playwright mode."
            </p>

            <div class="space-y-5">
                // Browser engine dropdown
                <div>
                    <label class="block text-sm font-medium text-text-primary mb-2">"Browser Engine"</label>
                    <select
                        on:change=move |ev| {
                            let val = event_target_value(&ev);
                            config.update(|c| c.browser_engine = val);
                            save_fn.with_value(|f| f());
                        }
                        prop:value=move || config.get().browser_engine
                        class="block w-full max-w-xs px-3 py-2 bg-surface border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                    >
                        <option value="chromium" selected=move || config.get().browser_engine == "chromium">"Chromium (default)"</option>
                        <option value="chrome" selected=move || config.get().browser_engine == "chrome">"Google Chrome"</option>
                        <option value="edge" selected=move || config.get().browser_engine == "edge">"Microsoft Edge"</option>
                        <option value="brave" selected=move || config.get().browser_engine == "brave">"Brave"</option>
                    </select>
                    <p class="mt-1 text-xs text-text-tertiary">"The browser engine used for headless automation. Chromium is bundled with Playwright."</p>
                </div>

                // Headless toggle
                <div class="flex items-center justify-between">
                    <div>
                        <div class="font-medium text-text-primary">"Headless Mode"</div>
                        <div class="text-sm text-text-tertiary">"Run without a visible window. Disable to watch browser actions in real time."</div>
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
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Chrome DevTools Settings"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure how Chrome DevTools mode connects to a browser."
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
                        <div class="font-medium text-text-primary">"Your Chrome"</div>
                        <div class="text-sm text-text-tertiary">"Connect to your personal Chrome browser. Access all logged-in sessions, cookies, and extensions."</div>
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
                        <div class="font-medium text-text-primary">"Isolated Instance"</div>
                        <div class="text-sm text-text-tertiary">"Launch a clean Chrome instance managed by Aleph. No pre-existing sessions or cookies."</div>
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
    let (saving, save_error, save_success, save_fn) = section_save!(config);

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Security"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Network security settings for all browser modes."
            </p>

            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <div>
                        <div class="font-medium text-text-primary">"Block Private Networks"</div>
                        <div class="text-sm text-text-tertiary">"Prevent access to private/internal addresses (127.0.0.1, 10.x, 192.168.x, etc.)."</div>
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

                <SaveFeedback saving=saving save_error=save_error save_success=save_success />
            </div>
        </div>
    }
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
    view! {
        {move || {
            if saving.get() {
                Some(view! {
                    <div class="text-sm text-text-tertiary">"Saving..."</div>
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
                        "Saved."
                    </div>
                }.into_any())
            } else {
                None
            }
        }}
    }
}
