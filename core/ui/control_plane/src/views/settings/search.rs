//! Search Settings View
//!
//! Split-pane layout matching AI Providers/Embedding/Generation Providers:
//! - Left panel: Preset search provider grid + global search settings
//! - Right panel: Detail panel for selected provider

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{SearchBackendEntry, SearchConfig, SearchConfigApi};

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
    needs_engine_id: bool,
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
        needs_engine_id: false,
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
        needs_engine_id: false,
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
        needs_engine_id: true,
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
        needs_engine_id: false,
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
        needs_engine_id: false,
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
        needs_engine_id: false,
    },
];

fn find_preset(name: &str) -> Option<&'static SearchPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Find backend entry for a provider name from the config's backends list
fn find_backend<'a>(backends: &'a [SearchBackendEntry], name: &str) -> Option<&'a SearchBackendEntry> {
    backends.iter().find(|b| b.name == name)
}

// ============================================================================
// Main View
// ============================================================================

#[component]
pub fn SearchView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(SearchConfig {
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
    let error = RwSignal::new(Option::<String>::None);
    let selected = RwSignal::new(Option::<String>::None);
    let show_add_form = RwSignal::new(false);

    // Load config on mount
    spawn_local(async move {
        match SearchConfigApi::get(&state).await {
            Ok(cfg) => {
                // Only auto-select if there's an active provider
                if !cfg.default_provider.is_empty() {
                    selected.set(Some(cfg.default_provider.clone()));
                }
                config.set(cfg);
                error.set(None);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load config: {}", e)));
            }
        }
        loading.set(false);
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
                    <PresetGrid config=config selected=selected show_add_form=show_add_form />

                    // Add Custom Provider button
                    <div class="pt-2">
                        <button
                            on:click=move |_| {
                                show_add_form.set(true);
                                selected.set(None);
                            }
                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                        >
                            "+ Add Custom Provider"
                        </button>
                    </div>

                    // Global search settings
                    <GlobalSettings config=config loading=loading />
                </div>
            </div>

            // Right panel: Detail or Add form
            <div class="w-7/12 min-w-0 overflow-y-auto">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddCustomSearchProviderPanel
                                config=config
                                on_added=move || {
                                    show_add_form.set(false);
                                }
                                on_cancel=move || show_add_form.set(false)
                            />
                        }.into_any()
                    } else {
                        view! {
                            <ProviderDetailPanel config=config selected=selected error=error />
                        }.into_any()
                    }
                }}
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
    show_add_form: RwSignal<bool>,
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

                    let is_active = move || {
                        let dp = config.get().default_provider;
                        !dp.is_empty() && dp == name
                    };

                    let on_click = move |_| {
                        selected.set(Some(name.to_string()));
                        show_add_form.set(false);
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
                                        {move || {
                                            let cfg = config.get();
                                            let is_default = !cfg.default_provider.is_empty() && cfg.default_provider == name;
                                            let backend_verified = cfg.backends.iter().find(|b| b.name == name).map_or(false, |b| b.verified);
                                            if is_default {
                                                view! {
                                                    <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                        "Default"
                                                    </span>
                                                }.into_any()
                                            } else if backend_verified {
                                                view! {
                                                    <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                        "Verified"
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }
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
                    let provider_display = if cfg.default_provider.is_empty() {
                        "None".to_string()
                    } else {
                        cfg.default_provider.clone()
                    };
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
                                <span>"Provider: " {provider_display}</span>
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

    // Per-provider backend fields
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_engine_id = RwSignal::new(String::new());

    let saving = RwSignal::new(false);
    let save_success = RwSignal::new(false);
    let testing = RwSignal::new(false);
    let test_success = RwSignal::new(Option::<bool>::None);
    let deleting = RwSignal::new(false);

    // Sync form when config or selection changes
    Effect::new(move || {
        let sel = selected.get();
        let cfg = config.get();
        form_enabled.set(cfg.enabled);
        form_max_results.set(cfg.max_results);
        form_timeout.set(cfg.timeout_seconds);

        // Load per-provider backend fields
        if let Some(sel_name) = &sel {
            if let Some(backend) = find_backend(&cfg.backends, sel_name) {
                form_api_key.set(backend.api_key.clone().unwrap_or_default());
                form_base_url.set(backend.base_url.clone().unwrap_or_default());
                form_engine_id.set(backend.engine_id.clone().unwrap_or_default());
            } else {
                // No saved backend — use preset default base_url
                form_api_key.set(String::new());
                form_base_url.set(
                    find_preset(sel_name)
                        .map(|p| p.base_url.to_string())
                        .unwrap_or_default(),
                );
                form_engine_id.set(String::new());
            }
        }
    });

    /// Build updated backends list with the current provider's form values merged in
    fn build_backends(
        existing: &[SearchBackendEntry],
        provider_name: &str,
        api_key: String,
        base_url: String,
        engine_id: String,
    ) -> Vec<SearchBackendEntry> {
        let mut backends: Vec<SearchBackendEntry> = existing
            .iter()
            .filter(|b| b.name != provider_name)
            .cloned()
            .collect();
        backends.push(SearchBackendEntry {
            name: provider_name.to_string(),
            api_key: if api_key.is_empty() { None } else { Some(api_key) },
            base_url: if base_url.is_empty() { None } else { Some(base_url) },
            engine_id: if engine_id.is_empty() { None } else { Some(engine_id) },
            verified: false,
        });
        backends
    }

    let on_test = move |_| {
        let sel = selected.get();
        if sel.is_none() { return; }
        let provider_name = sel.unwrap();

        testing.set(true);
        test_success.set(None);
        error.set(None);

        let api_key = form_api_key.get();
        let base_url = form_base_url.get();
        let engine_id = form_engine_id.get();

        spawn_local(async move {
            match SearchConfigApi::test_connection(
                &state,
                &provider_name,
                if api_key.is_empty() { None } else { Some(api_key) },
                if base_url.is_empty() { None } else { Some(base_url) },
                if engine_id.is_empty() { None } else { Some(engine_id) },
            ).await {
                Ok(result) => {
                    test_success.set(Some(result.success));
                    if result.success {
                        // Refresh config to pick up verified=true
                        if let Ok(new_cfg) = SearchConfigApi::get(&state).await {
                            config.set(new_cfg);
                        }
                    }
                    if !result.success {
                        error.set(Some(result.message));
                    }
                    set_timeout(
                        move || test_success.set(None),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => {
                    test_success.set(Some(false));
                    error.set(Some(format!("Test failed: {}", e)));
                    set_timeout(
                        move || test_success.set(None),
                        std::time::Duration::from_secs(3),
                    );
                }
            }
            testing.set(false);
        });
    };

    let on_save = move |_| {
        let sel = selected.get();
        if sel.is_none() { return; }
        let provider_name = sel.unwrap();

        saving.set(true);
        error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.enabled = form_enabled.get();
        cfg.max_results = form_max_results.get();
        cfg.timeout_seconds = form_timeout.get();
        cfg.backends = build_backends(
            &cfg.backends,
            &provider_name,
            form_api_key.get(),
            form_base_url.get(),
            form_engine_id.get(),
        );

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
        cfg.default_provider = provider_name.clone();
        cfg.backends = build_backends(
            &cfg.backends,
            &provider_name,
            form_api_key.get(),
            form_base_url.get(),
            form_engine_id.get(),
        );

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

    let on_delete = move |_| {
        let sel = selected.get();
        if sel.is_none() { return; }
        let provider_name = sel.unwrap();

        deleting.set(true);
        error.set(None);

        spawn_local(async move {
            match SearchConfigApi::delete_backend(&state, &provider_name).await {
                Ok(()) => {
                    // Refresh config
                    if let Ok(new_cfg) = SearchConfigApi::get(&state).await {
                        config.set(new_cfg);
                    }
                    selected.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Delete failed: {}", e)));
                }
            }
            deleting.set(false);
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
                let is_active = {
                    let dp = config.get().default_provider;
                    !dp.is_empty() && dp == sel_name
                };
                let is_verified = config.get().backends.iter().find(|b| b.name == sel_name).map_or(false, |b| b.verified);

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
                                                <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded">
                                                    "Default"
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}
                                        {if is_verified {
                                            view! {
                                                <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded">
                                                    "Verified"
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
                            // Provider credentials
                            {if let Some(p) = preset {
                                let needs_api_key = p.needs_api_key;
                                let needs_engine_id = p.needs_engine_id;
                                let placeholder = p.api_key_placeholder;
                                let default_base_url = p.base_url;
                                let is_self_hosted = p.is_self_hosted;

                                view! {
                                    <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                        <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Provider Configuration"</h3>

                                        // API Key
                                        {if needs_api_key {
                                            view! {
                                                <div>
                                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                                        "API Key"
                                                    </label>
                                                    <input
                                                        type="password"
                                                        prop:value=move || form_api_key.get()
                                                        on:input=move |ev| form_api_key.set(event_target_value(&ev))
                                                        placeholder=placeholder
                                                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                                    />
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="flex items-center gap-2 text-sm text-text-tertiary">
                                                    <svg class="w-4 h-4 text-success" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                                    </svg>
                                                    "No API key required"
                                                </div>
                                            }.into_any()
                                        }}

                                        // Base URL
                                        <div>
                                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                                "Base URL"
                                            </label>
                                            <input
                                                type="text"
                                                prop:value=move || form_base_url.get()
                                                on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                                placeholder=default_base_url
                                                class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                            />
                                            {if is_self_hosted {
                                                view! {
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        "URL of your self-hosted instance"
                                                    </p>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        "Default: " {default_base_url}
                                                    </p>
                                                }.into_any()
                                            }}
                                        </div>

                                        // Engine ID (Google only)
                                        {if needs_engine_id {
                                            view! {
                                                <div>
                                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                                        "Search Engine ID"
                                                    </label>
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_engine_id.get()
                                                        on:input=move |ev| form_engine_id.set(event_target_value(&ev))
                                                        placeholder="Google Custom Search Engine ID"
                                                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                                    />
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        "Required for Google Custom Search"
                                                    </p>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}

                                        // Self-hosted badge
                                        {if is_self_hosted {
                                            view! {
                                                <div class="flex items-center gap-2">
                                                    <span class="px-2 py-0.5 bg-info-subtle text-info text-xs font-medium rounded">"Self-hosted"</span>
                                                    <span class="text-xs text-text-tertiary">"Runs on your infrastructure"</span>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}
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

                            // Test result
                            {move || test_success.get().map(|success| {
                                if success {
                                    view! {
                                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                            </svg>
                                            "Connection successful"
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm flex items-center gap-2">
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                            </svg>
                                            "Connection failed"
                                        </div>
                                    }.into_any()
                                }
                            })}

                            // Actions
                            <div class="space-y-2">
                                <div class="flex flex-row gap-3">
                                    <button
                                        on:click=on_test
                                        prop:disabled=move || testing.get()
                                        class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium text-sm"
                                    >
                                        {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                                    </button>

                                    <button
                                        on:click=on_save
                                        prop:disabled=move || saving.get()
                                        class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                    >
                                        {move || if saving.get() { "Saving..." } else { "Save" }}
                                    </button>
                                </div>

                                {let sel_name_for_row2 = sel_name.clone(); move || {
                                    let has_backend = config.get().backends.iter().any(|b| b.name == sel_name_for_row2);
                                    if has_backend {
                                        view! {
                                            <div class="flex flex-row gap-3">
                                                {if !is_active {
                                                    Some(view! {
                                                        <button
                                                            on:click=on_set_active
                                                            prop:disabled=move || saving.get()
                                                            class="flex-1 px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                                        >
                                                            "Set as Default"
                                                        </button>
                                                    })
                                                } else {
                                                    None
                                                }}
                                                {if !is_active {
                                                    Some(view! {
                                                        <button
                                                            on:click=on_delete
                                                            prop:disabled=move || deleting.get()
                                                            class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                        >
                                                            {move || if deleting.get() { "Deleting..." } else { "Delete" }}
                                                        </button>
                                                    })
                                                } else {
                                                    None
                                                }}
                                            </div>
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

// ============================================================================
// Add Custom Search Provider Panel
// ============================================================================

#[component]
fn AddCustomSearchProviderPanel(
    config: RwSignal<SearchConfig>,
    on_added: impl Fn() + 'static + Copy,
    on_cancel: impl Fn() + 'static + Copy,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let form_name = RwSignal::new(String::new());
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_engine_id = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let on_add = move |_| {
        let name = form_name.get().trim().to_string();
        if name.is_empty() {
            error.set(Some("Provider name is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let mut cfg = config.get();
        // Add backend entry
        cfg.backends.push(SearchBackendEntry {
            name: name.clone(),
            api_key: {
                let v = form_api_key.get();
                if v.is_empty() { None } else { Some(v) }
            },
            base_url: {
                let v = form_base_url.get();
                if v.is_empty() { None } else { Some(v) }
            },
            engine_id: {
                let v = form_engine_id.get();
                if v.is_empty() { None } else { Some(v) }
            },
            verified: false,
        });

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg.clone()).await {
                Ok(()) => {
                    config.set(cfg);
                    on_added();
                }
                Err(e) => {
                    error.set(Some(format!("Failed to add provider: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <h2 class="text-xl font-semibold text-text-primary">"Add Custom Provider"</h2>
                    <button
                        on:click=move |_| on_cancel()
                        class="text-text-tertiary hover:text-text-primary transition-colors"
                    >
                        "Cancel"
                    </button>
                </div>
            </div>

            // Form
            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                {move || error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                })}

                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Provider Details"</h3>

                    // Provider Name
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Provider Name"</label>
                        <input
                            type="text"
                            prop:value=move || form_name.get()
                            on:input=move |ev| form_name.set(event_target_value(&ev))
                            placeholder="e.g., my-searxng, custom-search"
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    // API Key
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"API Key"</label>
                        <input
                            type="password"
                            prop:value=move || form_api_key.get()
                            on:input=move |ev| form_api_key.set(event_target_value(&ev))
                            placeholder="Optional — leave empty if not required"
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                        />
                    </div>

                    // Base URL
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Base URL"</label>
                        <input
                            type="text"
                            prop:value=move || form_base_url.get()
                            on:input=move |ev| form_base_url.set(event_target_value(&ev))
                            placeholder="https://api.example.com/search"
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                        />
                    </div>

                    // Engine ID
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Engine ID"</label>
                        <input
                            type="text"
                            prop:value=move || form_engine_id.get()
                            on:input=move |ev| form_engine_id.set(event_target_value(&ev))
                            placeholder="Optional — for providers that require it"
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                        />
                    </div>
                </div>

                // Add button
                <button
                    on:click=on_add
                    prop:disabled=move || saving.get() || form_name.get().trim().is_empty()
                    class="w-full px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                >
                    {move || if saving.get() { "Adding..." } else { "Add Provider" }}
                </button>
            </div>
        </div>
    }
}
