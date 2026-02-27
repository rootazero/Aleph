//! Search Settings View
//!
//! Split-pane layout matching AI Providers/Embedding/Generation Providers:
//! - Left panel: Preset search provider grid + global search settings
//! - Right panel: Detail panel for selected provider

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{SearchConfig, SearchConfigApi};

// ============================================================================
// Preset Definitions
// ============================================================================

struct SearchPreset {
    name: &'static str,
    display_name: &'static str,
    description: &'static str,
    base_url: &'static str,
    api_key_placeholder: &'static str,
    icon_color: &'static str,
    needs_api_key: bool,
    is_self_hosted: bool,
}

const PRESETS: &[SearchPreset] = &[
    SearchPreset {
        name: "tavily",
        display_name: "Tavily",
        description: "AI-powered search API",
        base_url: "https://api.tavily.com",
        api_key_placeholder: "tvly-...",
        icon_color: "#5B5FC7",
        needs_api_key: true,
        is_self_hosted: false,
    },
    SearchPreset {
        name: "brave",
        display_name: "Brave",
        description: "Brave Search API",
        base_url: "https://api.search.brave.com/res/v1",
        api_key_placeholder: "BSA...",
        icon_color: "#FB542B",
        needs_api_key: true,
        is_self_hosted: false,
    },
    SearchPreset {
        name: "google",
        display_name: "Google",
        description: "Google Custom Search",
        base_url: "https://www.googleapis.com/customsearch/v1",
        api_key_placeholder: "AIza...",
        icon_color: "#4285F4",
        needs_api_key: true,
        is_self_hosted: false,
    },
    SearchPreset {
        name: "bing",
        display_name: "Bing",
        description: "Bing Web Search API",
        base_url: "https://api.bing.microsoft.com/v7.0",
        api_key_placeholder: "Ocp-Apim...",
        icon_color: "#008373",
        needs_api_key: true,
        is_self_hosted: false,
    },
    SearchPreset {
        name: "searxng",
        display_name: "SearXNG",
        description: "Self-hosted meta search",
        base_url: "http://localhost:8080",
        api_key_placeholder: "",
        icon_color: "#3050FF",
        needs_api_key: false,
        is_self_hosted: true,
    },
    SearchPreset {
        name: "exa",
        display_name: "Exa",
        description: "Neural search engine",
        base_url: "https://api.exa.ai",
        api_key_placeholder: "exa-...",
        icon_color: "#000000",
        needs_api_key: true,
        is_self_hosted: false,
    },
];

fn find_preset(name: &str) -> Option<&'static SearchPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

// ============================================================================
// Main View
// ============================================================================

#[component]
pub fn SearchView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(SearchConfig {
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
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let selected = RwSignal::new(Option::<String>::None);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match SearchConfigApi::get(&state).await {
                    Ok(cfg) => {
                        selected.set(Some(cfg.default_provider.clone()));
                        config.set(cfg);
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load config: {}", e)));
                    }
                }
                loading.set(false);
            });
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex h-full">
            // Left panel: Presets + Settings
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border">
                // Header
                <div class="px-6 py-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">"Search Providers"</h1>
                    <p class="mt-1 text-sm text-text-tertiary">
                        "Configure web search providers for AI-assisted research."
                    </p>
                </div>

                // Scrollable content
                <div class="flex-1 overflow-y-auto p-6 space-y-6">
                    {move || error.get().filter(|e| e.contains("Failed to load")).map(|_| view! {
                        <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <circle cx="12" cy="12" r="10"/>
                                <line x1="12" y1="16" x2="12" y2="12"/>
                                <line x1="12" y1="8" x2="12.01" y2="8"/>
                            </svg>
                            "Gateway not available — showing presets only"
                        </div>
                    })}

                    // Preset grid
                    <PresetGrid config=config selected=selected />

                    // Global search settings
                    <GlobalSettings config=config loading=loading />
                </div>
            </div>

            // Right panel: Detail
            <div class="w-7/12 min-w-0 overflow-y-auto">
                <ProviderDetailPanel config=config selected=selected error=error />
            </div>
        </div>
    }
}

// ============================================================================
// Preset Grid
// ============================================================================

#[component]
fn PresetGrid(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Search Providers"
            </h2>
            <div class="grid grid-cols-1 gap-2">
                {PRESETS.iter().map(|preset| {
                    let name = preset.name;
                    let display_name = preset.display_name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;
                    let first_char = preset.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();

                    let is_active = move || config.get().default_provider == name;

                    let on_click = move |_| {
                        selected.set(Some(name.to_string()));
                    };

                    view! {
                        <button
                            on:click=on_click
                            class=move || {
                                let base = "text-left p-3 rounded-lg border transition-all";
                                let sel = selected.get();
                                let is_sel = sel.as_deref() == Some(name);
                                if is_sel {
                                    format!("{} bg-primary-subtle border-primary", base)
                                } else if is_active() {
                                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                } else {
                                    format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                                }
                            }
                        >
                            <div class="flex items-center gap-3">
                                <div
                                    class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                    style=format!("background-color: {}", icon_color)
                                >
                                    {first_char}
                                </div>
                                <div class="min-w-0">
                                    <div class="flex items-center gap-2">
                                        <span class="font-medium text-text-primary text-sm truncate">
                                            {display_name}
                                        </span>
                                        {move || if is_active() {
                                            view! {
                                                <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                    "Active"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}
                                    </div>
                                    <div class="text-xs text-text-tertiary truncate">
                                        {description}
                                    </div>
                                </div>
                            </div>
                        </button>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

// ============================================================================
// Global Settings
// ============================================================================

#[component]
fn GlobalSettings(
    config: RwSignal<SearchConfig>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Search Settings"
            </h2>
            {move || {
                if loading.get() {
                    view! {
                        <div class="text-center py-4 text-text-tertiary text-sm">"Loading..."</div>
                    }.into_any()
                } else {
                    let cfg = config.get();
                    view! {
                        <div class="bg-surface-raised rounded-lg border border-border p-4 space-y-3">
                            <div class="flex items-center justify-between">
                                <div>
                                    <div class="text-sm font-medium text-text-primary">"Web Search"</div>
                                    <div class="text-xs text-text-tertiary">"Allow AI to search the web for information"</div>
                                </div>
                                <div class=move || {
                                    if config.get().enabled {
                                        "px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded"
                                    } else {
                                        "px-2 py-0.5 bg-surface-sunken text-text-tertiary text-xs font-medium rounded"
                                    }
                                }>
                                    {move || if config.get().enabled { "Enabled" } else { "Disabled" }}
                                </div>
                            </div>

                            <div class="flex items-center gap-4 text-xs text-text-tertiary">
                                <span>"Max Results: " {cfg.max_results}</span>
                                <span>"\u{00B7}"</span>
                                <span>"Timeout: " {cfg.timeout_seconds} "s"</span>
                                <span>"\u{00B7}"</span>
                                <span>"Provider: " {cfg.default_provider.clone()}</span>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// Detail Panel (Right Side)
// ============================================================================

#[component]
fn ProviderDetailPanel(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form state mirrors config for editing
    let form_enabled = RwSignal::new(false);
    let form_max_results = RwSignal::new(5u64);
    let form_timeout = RwSignal::new(10u64);

    let saving = RwSignal::new(false);
    let save_success = RwSignal::new(false);

    // Sync form when config or selection changes
    Effect::new(move || {
        let _ = selected.get(); // track
        let cfg = config.get();
        form_enabled.set(cfg.enabled);
        form_max_results.set(cfg.max_results);
        form_timeout.set(cfg.timeout_seconds);
    });

    let on_save = move |_| {
        let sel = selected.get();
        if sel.is_none() { return; }
        let provider_name = sel.unwrap();

        saving.set(true);
        error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.default_provider = provider_name;
        cfg.enabled = form_enabled.get();
        cfg.max_results = form_max_results.get();
        cfg.timeout_seconds = form_timeout.get();

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg.clone()).await {
                Ok(()) => {
                    config.set(cfg);
                    save_success.set(true);
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    let on_set_active = move |_| {
        let sel = selected.get();
        if sel.is_none() { return; }
        let provider_name = sel.unwrap();

        saving.set(true);
        error.set(None);

        let mut cfg = config.get();
        cfg.default_provider = provider_name;

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg.clone()).await {
                Ok(()) => {
                    config.set(cfg);
                    save_success.set(true);
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            {move || {
                let sel = selected.get();
                if sel.is_none() {
                    return view! {
                        <div class="flex flex-col items-center justify-center flex-1 text-text-tertiary">
                            <svg class="w-12 h-12 mb-3 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"
                                    d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                            </svg>
                            <span class="text-sm">"Select a search provider to configure"</span>
                        </div>
                    }.into_any();
                }

                let sel_name = sel.unwrap();
                let preset = find_preset(&sel_name);
                let is_active = config.get().default_provider == sel_name;

                view! {
                    <div class="flex flex-col h-full">
                        // Header
                        <div class="px-6 py-4 border-b border-border">
                            <div class="flex items-center gap-3">
                                {if let Some(p) = preset {
                                    let ch = p.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                    view! {
                                        <div
                                            class="w-10 h-10 rounded-xl flex items-center justify-center text-white font-bold"
                                            style=format!("background-color: {}", p.icon_color)
                                        >
                                            {ch}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="w-10 h-10 rounded-xl bg-surface-sunken flex items-center justify-center text-text-tertiary font-bold">
                                            "?"
                                        </div>
                                    }.into_any()
                                }}
                                <div class="flex-1">
                                    <div class="flex items-center gap-2">
                                        <h2 class="text-lg font-semibold text-text-primary">
                                            {preset.map(|p| p.display_name).unwrap_or(&sel_name)}
                                        </h2>
                                        {if is_active {
                                            view! {
                                                <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded">
                                                    "Active"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}
                                    </div>
                                    <p class="text-xs text-text-tertiary">
                                        {preset.map(|p| p.description).unwrap_or("Search provider")}
                                    </p>
                                </div>
                            </div>
                        </div>

                        // Content
                        <div class="flex-1 overflow-y-auto p-6 space-y-6">
                            // Provider info
                            {if let Some(p) = preset {
                                view! {
                                    <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                                        <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Provider Details"</h3>
                                        <div class="space-y-2">
                                            <div class="flex items-center justify-between">
                                                <span class="text-sm text-text-secondary">"Base URL"</span>
                                                <span class="text-sm text-text-primary font-mono">{p.base_url}</span>
                                            </div>
                                            <div class="flex items-center justify-between">
                                                <span class="text-sm text-text-secondary">"API Key Required"</span>
                                                <span class="text-sm text-text-primary">{if p.needs_api_key { "Yes" } else { "No" }}</span>
                                            </div>
                                            {if p.is_self_hosted {
                                                view! {
                                                    <div class="flex items-center justify-between">
                                                        <span class="text-sm text-text-secondary">"Type"</span>
                                                        <span class="px-2 py-0.5 bg-info-subtle text-info text-xs font-medium rounded">"Self-hosted"</span>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }}
                                        </div>
                                    </div>
                                }.into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}

                            // Error
                            {move || error.get().filter(|e| !e.contains("Failed to load")).map(|e| view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                            })}

                            // Search Settings
                            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-5">
                                <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Search Settings"</h3>

                                // Enabled
                                <label class="flex items-center gap-3 cursor-pointer">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || form_enabled.get()
                                        on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                                        class="w-4 h-4 rounded"
                                    />
                                    <div>
                                        <span class="text-sm text-text-primary">"Enable Search"</span>
                                        <p class="text-xs text-text-tertiary">"Allow AI to search the web for information"</p>
                                    </div>
                                </label>

                                // Max Results
                                <div>
                                    <div class="flex items-center justify-between mb-2">
                                        <label class="text-sm text-text-secondary">"Max Results"</label>
                                        <span class="text-sm text-text-primary font-mono">{move || form_max_results.get()}</span>
                                    </div>
                                    <input
                                        type="range"
                                        min="1"
                                        max="20"
                                        step="1"
                                        prop:value=move || form_max_results.get()
                                        on:input=move |ev| {
                                            if let Ok(val) = event_target_value(&ev).parse::<u64>() {
                                                form_max_results.set(val);
                                            }
                                        }
                                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                    />
                                    <div class="flex justify-between text-xs text-text-tertiary mt-1">
                                        <span>"1"</span>
                                        <span>"20"</span>
                                    </div>
                                </div>

                                // Timeout
                                <div>
                                    <div class="flex items-center justify-between mb-2">
                                        <label class="text-sm text-text-secondary">"Timeout"</label>
                                        <span class="text-sm text-text-primary font-mono">{move || form_timeout.get()} "s"</span>
                                    </div>
                                    <input
                                        type="range"
                                        min="5"
                                        max="60"
                                        step="5"
                                        prop:value=move || form_timeout.get()
                                        on:input=move |ev| {
                                            if let Ok(val) = event_target_value(&ev).parse::<u64>() {
                                                form_timeout.set(val);
                                            }
                                        }
                                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                    />
                                    <div class="flex justify-between text-xs text-text-tertiary mt-1">
                                        <span>"5s"</span>
                                        <span>"60s"</span>
                                    </div>
                                </div>
                            </div>

                            // Save success
                            {move || save_success.get().then(|| view! {
                                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                    </svg>
                                    "Saved successfully"
                                </div>
                            })}

                            // Actions
                            <div class="space-y-2">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="w-full px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save Settings" }}
                                </button>

                                {move || {
                                    if !is_active {
                                        view! {
                                            <button
                                                on:click=on_set_active
                                                prop:disabled=move || saving.get()
                                                class="w-full px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                            >
                                                "Set as Active Provider"
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }
                                }}
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
