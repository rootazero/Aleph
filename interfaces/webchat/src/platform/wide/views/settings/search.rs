//! Search Settings View
//!
//! Split-pane layout matching AI Providers/Embedding/Generation Providers:
//! - Left panel: Preset search provider grid + global search settings
//! - Right panel: Detail panel for selected provider

use crate::api::{
    FetchBackendEntry, FetchConfig, FetchConfigApi, SearchBackendEntry, SearchConfig,
    SearchConfigApi,
};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

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
    SearchPreset {
        name: "firecrawl",
        display_name: "Firecrawl",
        description: "Search + full-content scraping",
        base_url: "https://api.firecrawl.dev",
        api_key_placeholder: "fc-...",
        icon_color: "#FF6B35",
        needs_api_key: true,
        is_self_hosted: false,
        needs_engine_id: false,
    },
    SearchPreset {
        name: "duckduckgo",
        display_name: "DuckDuckGo",
        description: "No-account HTML search",
        base_url: "",
        api_key_placeholder: "",
        icon_color: "#DE5833",
        needs_api_key: false,
        is_self_hosted: false,
        needs_engine_id: false,
    },
];

fn find_preset(name: &str) -> Option<&'static SearchPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Find backend entry for a provider name from the config's backends list
fn find_backend<'a>(
    backends: &'a [SearchBackendEntry],
    name: &str,
) -> Option<&'a SearchBackendEntry> {
    backends.iter().find(|b| b.name == name)
}

// ============================================================================
// Main View
// ============================================================================

#[component]
#[must_use]
pub fn SearchView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

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
                error.set(Some(format!("Failed to load config: {e}")));
            }
        }
        loading.set(false);
    });

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel: Presets + Settings
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border aleph-md-list">
                // Header
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">{t!(i18n, settings.search.title)}</h1>
                    <p class="mt-1 text-sm text-text-tertiary">
                        {t!(i18n, settings.search.description)}
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
                            {t!(i18n, settings.search.gateway_unavailable)}
                        </div>
                    })}

                    // Preset grid
                    <PresetGrid config=config selected=selected show_add_form=show_add_form />

                    // Custom providers (not matching any preset)
                    <CustomSearchProvidersList config=config selected=selected show_add_form=show_add_form />

                    // Add Custom Provider button
                    <div class="pt-2">
                        <button
                            on:click=move |_| {
                                show_add_form.set(true);
                                selected.set(None);
                            }
                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                        >
                            {t!(i18n, settings.search.add_custom)}
                        </button>
                    </div>

                    // Global search settings
                    <GlobalSettings config=config loading=loading />

                    // Fetch providers (crawl4ai + shared Firecrawl)
                    <FetchProvidersSection />
                </div>
            </div>

            // Right panel: Detail or Add form
            <div class="w-7/12 min-w-0 overflow-y-auto aleph-md-detail">
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
    let i18n = use_i18n();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.search.providers_section)}
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
                                    format!("{base} bg-primary-subtle border-primary")
                                } else if is_active() {
                                    format!("{base} bg-surface-raised border-border hover:border-primary/40")
                                } else {
                                    format!("{base} bg-surface-sunken border-border hover:border-border-strong")
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
                                            let backend_verified = cfg.backends.iter().find(|b| b.name == name).is_some_and(|b| b.verified);
                                            view! {
                                                <ProviderBadges state=BadgeState {
                                                    is_default,
                                                    verified: backend_verified,
                                                } />
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
// Custom Search Providers List (non-preset providers)
// ============================================================================

#[component]
fn CustomSearchProvidersList(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let preset_names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();

    view! {
        {move || {
            let cfg = config.get();
            let custom: Vec<_> = cfg.backends.iter()
                .filter(|b| !preset_names.contains(&b.name.as_str()))
                .cloned()
                .collect();
            if custom.is_empty() {
                view! { <div></div> }.into_any()
            } else {
                view! {
                    <div>
                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                            {t!(i18n, settings.search.custom_providers)}
                        </h2>
                        <div class="grid grid-cols-1 gap-2">
                            {custom.into_iter().map(|backend| {
                                let name = backend.name.clone();
                                let name_click = name.clone();
                                let name_check = name.clone();
                                let is_default = !cfg.default_provider.is_empty() && cfg.default_provider == name;
                                let verified = backend.verified;
                                let first_char = name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                view! {
                                    <button
                                        on:click=move |_| {
                                            selected.set(Some(name_click.clone()));
                                            show_add_form.set(false);
                                        }
                                        class=move || {
                                            let base = "text-left p-3 rounded-lg border transition-all";
                                            let is_sel = selected.get().as_deref() == Some(&name_check);
                                            if is_sel {
                                                format!("{base} bg-primary-subtle border-primary")
                                            } else {
                                                format!("{base} bg-surface-raised border-border hover:border-primary/40")
                                            }
                                        }
                                    >
                                        <div class="flex items-center gap-3">
                                            <div
                                                class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                style="background-color: #808080"
                                            >
                                                {first_char}
                                            </div>
                                            <div class="min-w-0">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-medium text-text-primary text-sm truncate">
                                                        {name}
                                                    </span>
                                                    <ProviderBadges state=BadgeState {
                                                        is_default,
                                                        verified,
                                                    } />
                                                </div>
                                                <div class="text-xs text-text-tertiary truncate">
                                                    {t!(i18n, settings.search.custom_search_provider)}
                                                </div>
                                            </div>
                                        </div>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}

// ============================================================================
// Global Settings
// ============================================================================

#[component]
fn GlobalSettings(config: RwSignal<SearchConfig>, loading: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.search.global_settings)}
            </h2>
            {move || {
                if loading.get() {
                    view! {
                        <div class="text-center py-4 text-text-tertiary text-sm">{t_string!(i18n, common.loading).to_string()}</div>
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
                                    <div class="text-sm font-medium text-text-primary">{t!(i18n, settings.search.web_search)}</div>
                                    <div class="text-xs text-text-tertiary">{t!(i18n, settings.search.web_search_desc)}</div>
                                </div>
                                <div class=move || {
                                    if config.get().enabled {
                                        "px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded"
                                    } else {
                                        "px-2 py-0.5 bg-surface-sunken text-text-tertiary text-xs font-medium rounded"
                                    }
                                }>
                                    {move || if config.get().enabled { t_string!(i18n, settings.search.enabled).to_string() } else { t_string!(i18n, settings.search.disabled).to_string() }}
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
    let i18n = use_i18n();

    // Form state mirrors config for editing
    let form_enabled = RwSignal::new(false);
    let form_max_results = RwSignal::new(5u64);
    let form_timeout = RwSignal::new(10u64);

    // Per-provider backend fields
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_engine_id = RwSignal::new(String::new());
    // SearXNG only — comma-separated engines to pin (e.g. "bing").
    let form_engines = RwSignal::new(String::new());
    // Whether the selected backend already has a key in the vault. The secret is
    // never echoed; the field starts empty and an empty value on save keeps it.
    let provider_has_key = RwSignal::new(false);

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

        // Load per-provider backend fields. The secret is never echoed: the key
        // field always starts empty; `has_api_key` only drives the status hint.
        if let Some(sel_name) = &sel {
            if let Some(backend) = find_backend(&cfg.backends, sel_name) {
                form_api_key.set(String::new());
                provider_has_key.set(backend.has_api_key);
                form_base_url.set(backend.base_url.clone().unwrap_or_default());
                form_engine_id.set(backend.engine_id.clone().unwrap_or_default());
                form_engines.set(backend.engines.clone().unwrap_or_default());
            } else {
                // No saved backend — use preset default base_url
                form_api_key.set(String::new());
                provider_has_key.set(false);
                form_base_url.set(
                    find_preset(sel_name)
                        .map(|p| p.base_url.to_string())
                        .unwrap_or_default(),
                );
                form_engine_id.set(String::new());
                form_engines.set(String::new());
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
        engines: String,
    ) -> Vec<SearchBackendEntry> {
        let mut backends: Vec<SearchBackendEntry> = existing
            .iter()
            .filter(|b| b.name != provider_name)
            .cloned()
            .collect();
        // A provider that was never configured and whose form is entirely
        // empty has nothing to save — pushing it anyway would create a
        // phantom backend entry as a side effect of merely selecting the
        // provider card (removal is the delete button's job, not save's).
        let has_existing = existing.iter().any(|b| b.name == provider_name);
        if !has_existing
            && api_key.is_empty()
            && base_url.is_empty()
            && engine_id.is_empty()
            && engines.is_empty()
        {
            return backends;
        }
        backends.push(SearchBackendEntry {
            name: provider_name.to_string(),
            api_key: if api_key.is_empty() {
                None
            } else {
                Some(api_key)
            },
            base_url: if base_url.is_empty() {
                None
            } else {
                Some(base_url)
            },
            engine_id: if engine_id.is_empty() {
                None
            } else {
                Some(engine_id)
            },
            engines: if engines.is_empty() {
                None
            } else {
                Some(engines)
            },
            has_api_key: false,
            verified: false,
        });
        backends
    }

    let on_test = move |_| {
        let Some(provider_name) = selected.get() else {
            return;
        };

        testing.set(true);
        test_success.set(None);
        error.set(None);

        let api_key = form_api_key.get();
        let base_url = form_base_url.get();
        let engine_id = form_engine_id.get();
        let engines = form_engines.get();

        spawn_local(async move {
            match SearchConfigApi::test_connection(
                &state,
                &provider_name,
                if api_key.is_empty() {
                    None
                } else {
                    Some(api_key)
                },
                if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                },
                if engine_id.is_empty() {
                    None
                } else {
                    Some(engine_id)
                },
                if engines.is_empty() {
                    None
                } else {
                    Some(engines)
                },
            )
            .await
            {
                Ok(result) => {
                    test_success.set(Some(result.success));
                    if result.success {
                        // Refresh config to pick up verified=true, but preserve
                        // the API key the user just typed so it is not wiped
                        // from the form before Save. The form Effect runs after
                        // the config update, so restore the key on the next tick.
                        let key = form_api_key.get();
                        if let Ok(new_cfg) = SearchConfigApi::get(&state).await {
                            config.set(new_cfg);
                            set_timeout(
                                move || form_api_key.set(key),
                                std::time::Duration::from_secs(0),
                            );
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
                    error.set(Some(format!("Test failed: {e}")));
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
        let Some(provider_name) = selected.get() else {
            return;
        };

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
            form_engines.get(),
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
                    error.set(Some(format!("Failed to save: {e}")));
                }
            }
            saving.set(false);
        });
    };

    let on_set_active = move |_| {
        let Some(provider_name) = selected.get() else {
            return;
        };

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
            form_engines.get(),
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
                    error.set(Some(format!("Failed to save: {e}")));
                }
            }
            saving.set(false);
        });
    };

    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
        let Some(provider_name) = selected.get() else {
            return;
        };

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
                    error.set(Some(format!("Delete failed: {e}")));
                }
            }
            deleting.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            {move || {
                let Some(sel_name) = selected.get() else {
                    return view! {
                        <div class="flex flex-col items-center justify-center flex-1 text-text-tertiary">
                            <svg class="w-12 h-12 mb-3 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"
                                    d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
                            </svg>
                            <span class="text-sm">{t!(i18n, settings.search.select_to_configure)}</span>
                        </div>
                    }.into_any();
                };
                let preset = find_preset(&sel_name);
                let is_active = {
                    let dp = config.get().default_provider;
                    !dp.is_empty() && dp == sel_name
                };
                let is_verified = config.get().backends.iter().find(|b| b.name == sel_name).is_some_and(|b| b.verified);

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
                                        <ProviderBadges state=BadgeState {
                                            is_default: is_active,
                                            verified: is_verified,
                                        } />
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
                                        <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.search.provider_config)}</h3>

                                        // API Key
                                        {if needs_api_key {
                                            view! {
                                                <div>
                                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                                        {t!(i18n, settings.search.api_key)}
                                                    </label>
                                                    <ProviderKeyField
                                                        value=form_api_key
                                                        has_api_key=provider_has_key.into()
                                                        hint=placeholder.to_string()
                                                    />
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="flex items-center gap-2 text-sm text-text-tertiary">
                                                    <svg class="w-4 h-4 text-success" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                                    </svg>
                                                    {t!(i18n, settings.search.no_api_key)}
                                                </div>
                                            }.into_any()
                                        }}

                                        // Base URL
                                        <div>
                                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                                {t!(i18n, settings.search.base_url)}
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
                                                        {t!(i18n, settings.search.base_url_hint_self_hosted)}
                                                    </p>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        {default_base_url}
                                                    </p>
                                                }.into_any()
                                            }}
                                        </div>

                                        // Engine ID (Google only)
                                        {if needs_engine_id {
                                            view! {
                                                <div>
                                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                                        {t!(i18n, settings.search.engine_id)}
                                                    </label>
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_engine_id.get()
                                                        on:input=move |ev| form_engine_id.set(event_target_value(&ev))
                                                        placeholder="Google Custom Search Engine ID"
                                                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                                    />
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        {t!(i18n, settings.search.engine_id_hint)}
                                                    </p>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}

                                        // Engines (SearXNG only) — pin upstream engines to dodge rate-limited ones
                                        {if p.name == "searxng" {
                                            view! {
                                                <div>
                                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                                        "Engines"
                                                    </label>
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_engines.get()
                                                        on:input=move |ev| form_engines.set(event_target_value(&ev))
                                                        placeholder="bing,brave  ·  leave empty for SearXNG defaults"
                                                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                                    />
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        "逗号分隔的上游引擎，把搜索钉在抗限流引擎上（如 bing,baidu），避免某引擎被限流/被墙导致空结果。Comma-separated upstream engines to pin."
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
                                                    <span class="px-2 py-0.5 bg-info-subtle text-info text-xs font-medium rounded">{t!(i18n, settings.search.self_hosted)}</span>
                                                    <span class="text-xs text-text-tertiary">{t!(i18n, settings.search.self_hosted_desc)}</span>
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
                                <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.search.settings_section)}</h3>

                                // Enabled
                                <label class="flex items-center gap-3 cursor-pointer">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || form_enabled.get()
                                        on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                                        class="w-4 h-4 rounded"
                                    />
                                    <div>
                                        <span class="text-sm text-text-primary">{t!(i18n, settings.search.enable_search)}</span>
                                        <p class="text-xs text-text-tertiary">{t!(i18n, settings.search.enable_search_desc)}</p>
                                    </div>
                                </label>

                                // Max Results
                                <div>
                                    <div class="flex items-center justify-between mb-2">
                                        <label class="text-sm text-text-secondary">{t!(i18n, settings.search.max_results)}</label>
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
                                        <label class="text-sm text-text-secondary">{t!(i18n, settings.search.timeout)}</label>
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
                                        {move || if testing.get() { t_string!(i18n, settings.search.testing).to_string() } else { t_string!(i18n, settings.search.test_connection).to_string() }}
                                    </button>

                                    <button
                                        on:click=on_save
                                        prop:disabled=move || saving.get()
                                        class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                    >
                                        {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                </div>

                                {let sel_name_for_row2 = sel_name.clone();
                                 let is_custom = preset.is_none();
                                 move || {
                                    let has_backend = config.get().backends.iter().any(|b| b.name == sel_name_for_row2);
                                    if has_backend {
                                        view! {
                                            <div class="flex flex-col gap-2">
                                                <div class="flex flex-row gap-3">
                                                    {if !is_active {
                                                        Some(view! {
                                                            <button
                                                                on:click=on_set_active
                                                                prop:disabled=move || saving.get() || !is_verified
                                                                class="flex-1 px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                                            >
                                                                {t!(i18n, settings.search.set_as_default)}
                                                            </button>
                                                        })
                                                    } else {
                                                        None
                                                    }}
                                                    {if !is_active && is_custom {
                                                        Some(view! {
                                                            {move || if confirming.get() {
                                                                view! {
                                                                    <ConfirmButton confirming=confirming on_confirm=on_confirm_delete width_class="flex-1" />
                                                                }.into_any()
                                                            } else {
                                                                view! {
                                                                    <button
                                                                        on:click=move |_| confirming.set(true)
                                                                        prop:disabled=move || deleting.get()
                                                                        class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                                    >
                                                                        {move || if deleting.get() { t_string!(i18n, settings.search.deleting).to_string() } else { t_string!(i18n, common.delete).to_string() }}
                                                                    </button>
                                                                }.into_any()
                                                            }}
                                                        })
                                                    } else {
                                                        None
                                                    }}
                                                </div>
                                                {(!is_active && !is_verified).then(|| view! {
                                                    <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.verify_before_default)}</p>
                                                })}
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
    let i18n = use_i18n();

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
            name,
            api_key: {
                let v = form_api_key.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            base_url: {
                let v = form_base_url.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            engine_id: {
                let v = form_engine_id.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            engines: None,
            has_api_key: false,
            verified: false,
        });

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg.clone()).await {
                Ok(()) => {
                    config.set(cfg);
                    on_added();
                }
                Err(e) => {
                    error.set(Some(format!("Failed to add provider: {e}")));
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
                    <h2 class="text-xl font-semibold text-text-primary">{t!(i18n, settings.search.add_custom_provider)}</h2>
                    <button
                        on:click=move |_| on_cancel()
                        class="text-text-tertiary hover:text-text-primary transition-colors"
                    >
                        {t!(i18n, common.cancel)}
                    </button>
                </div>
            </div>

            // Form
            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                {move || error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                })}

                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.search.provider_details)}</h3>

                    // Provider Name
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.provider_name)}</label>
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
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.api_key)}</label>
                        <ProviderKeyField
                            value=form_api_key
                            has_api_key=Signal::derive(|| false)
                            hint=t_string!(i18n, settings.search.optional_api_key).to_string()
                        />
                    </div>

                    // Base URL
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.base_url)}</label>
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
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.engine_id)}</label>
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
                    {move || if saving.get() { t_string!(i18n, settings.search.adding).to_string() } else { t_string!(i18n, settings.search.add_provider).to_string() }}
                </button>
            </div>
        </div>
    }
}

// ============================================================================
// Fetch Providers Section
// ============================================================================

/// Self-contained settings section for URL→Markdown fetch backends.
///
/// Renders below the search providers in the left panel (not entangled with the
/// search master-detail). Manages its own FetchConfig signal and form state.
#[component]
fn FetchProvidersSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let fetch_config = RwSignal::new(FetchConfig {
        enabled: false,
        default_provider: String::new(),
        backends: Vec::new(),
    });

    // Form signals for the crawl4ai card
    let form_enabled = RwSignal::new(false);
    let form_default_provider = RwSignal::new(String::from("crawl4ai"));
    let form_base_url = RwSignal::new(String::new());
    let form_api_key = RwSignal::new(String::new()); // write-only; never pre-filled
    let form_has_api_key = RwSignal::new(false);
    let form_timeout = RwSignal::new(30u64);

    // Save / Test state
    let saving = RwSignal::new(false);
    let save_success = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let testing = RwSignal::new(false);
    let test_result = RwSignal::new(Option::<(bool, String)>::None);

    // Firecrawl test state (separate from crawl4ai)
    let fc_testing = RwSignal::new(false);
    let fc_test_result = RwSignal::new(Option::<(bool, String)>::None);

    // Load on mount; silently degrade if server unavailable
    spawn_local(async move {
        if let Ok(cfg) = FetchConfigApi::get(&state).await {
            form_enabled.set(cfg.enabled);
            form_default_provider.set(if cfg.default_provider.is_empty() {
                "crawl4ai".to_string()
            } else {
                cfg.default_provider.clone()
            });
            if let Some(b) = cfg.backends.iter().find(|b| b.name == "crawl4ai") {
                form_base_url.set(b.base_url.clone().unwrap_or_default());
                form_has_api_key.set(b.has_api_key);
                form_timeout.set(b.timeout_seconds.unwrap_or(30));
            }
            fetch_config.set(cfg);
        }
    });

    // Section-level settings (enabled + default_provider) save on change. Each
    // sends the full config, preserving the persisted backends. Strategy V: the
    // synthesized firecrawl entry is filtered out so it is never written to
    // [fetch].backends.
    let persist_section = move || {
        let enabled = form_enabled.get();
        let default_provider = form_default_provider.get();
        spawn_local(async move {
            let cur = fetch_config.get();
            let backends: Vec<FetchBackendEntry> = cur
                .backends
                .iter()
                .filter(|b| b.name != "firecrawl")
                .map(|b| FetchBackendEntry {
                    name: b.name.clone(),
                    provider_type: b.provider_type.clone(),
                    base_url: b.base_url.clone(),
                    timeout_seconds: b.timeout_seconds,
                    api_key: None, // vault is the source; never re-send
                    has_api_key: false,
                    verified: false,
                    shares_search: b.shares_search,
                })
                .collect();
            let new_cfg = FetchConfig {
                enabled,
                default_provider,
                backends,
            };
            if FetchConfigApi::update(&state, new_cfg).await.is_ok() {
                if let Ok(refreshed) = FetchConfigApi::get(&state).await {
                    fetch_config.set(refreshed);
                }
            }
        });
    };

    let on_toggle_enabled = move |ev: web_sys::Event| {
        form_enabled.set(event_target_checked(&ev));
        persist_section();
    };
    let on_select_crawl4ai = move |_| {
        form_default_provider.set("crawl4ai".to_string());
        persist_section();
    };
    let on_select_firecrawl = move |_| {
        form_default_provider.set("firecrawl".to_string());
        persist_section();
    };

    // ── Save handler ─────────────────────────────────────────────────────────
    let on_save = move |_| {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let enabled = form_enabled.get();
        let base_url = form_base_url.get();
        let api_key = form_api_key.get();
        let timeout = form_timeout.get();

        spawn_local(async move {
            let old_cfg = fetch_config.get();
            // Keep other backends; drop crawl4ai (re-pushed below) and the
            // synthesized firecrawl entry (Strategy V — never persisted to [fetch]).
            let mut backends: Vec<FetchBackendEntry> = old_cfg
                .backends
                .into_iter()
                .filter(|b| b.name != "crawl4ai" && b.name != "firecrawl")
                .collect();
            backends.push(FetchBackendEntry {
                name: "crawl4ai".to_string(),
                provider_type: "crawl4ai".to_string(),
                base_url: if base_url.is_empty() {
                    None
                } else {
                    Some(base_url)
                },
                timeout_seconds: Some(timeout),
                api_key: if api_key.is_empty() {
                    None
                } else {
                    Some(api_key)
                },
                has_api_key: false,
                verified: false,
                shares_search: false,
            });
            let new_cfg = FetchConfig {
                enabled,
                default_provider: form_default_provider.get(),
                backends,
            };
            match FetchConfigApi::update(&state, new_cfg).await {
                Ok(()) => {
                    // Re-fetch to pick up server-side has_api_key / verified
                    if let Ok(refreshed) = FetchConfigApi::get(&state).await {
                        if let Some(b) = refreshed.backends.iter().find(|b| b.name == "crawl4ai") {
                            form_has_api_key.set(b.has_api_key);
                        }
                        form_api_key.set(String::new());
                        fetch_config.set(refreshed);
                    }
                    save_success.set(true);
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => {
                    save_error.set(Some(format!("Failed to save: {e}")));
                }
            }
            saving.set(false);
        });
    };

    // ── Test crawl4ai handler ─────────────────────────────────────────────────
    let on_test = move |_| {
        testing.set(true);
        test_result.set(None);

        let base_url = form_base_url.get();
        let api_key = form_api_key.get();
        let timeout = form_timeout.get();

        let entry = FetchBackendEntry {
            name: "crawl4ai".to_string(),
            provider_type: "crawl4ai".to_string(),
            base_url: if base_url.is_empty() {
                None
            } else {
                Some(base_url)
            },
            timeout_seconds: Some(timeout),
            api_key: if api_key.is_empty() {
                None
            } else {
                Some(api_key)
            },
            has_api_key: false,
            verified: false,
            shares_search: false,
        };

        spawn_local(async move {
            match FetchConfigApi::test_connection(&state, &entry).await {
                Ok(result) => {
                    let success = result.success;
                    let msg = result.message;
                    if success {
                        if let Ok(refreshed) = FetchConfigApi::get(&state).await {
                            if let Some(b) =
                                refreshed.backends.iter().find(|b| b.name == "crawl4ai")
                            {
                                form_has_api_key.set(b.has_api_key);
                            }
                            fetch_config.set(refreshed);
                        }
                    }
                    test_result.set(Some((success, msg)));
                    set_timeout(
                        move || test_result.set(None),
                        std::time::Duration::from_secs(5),
                    );
                }
                Err(e) => {
                    test_result.set(Some((false, format!("Test failed: {e}"))));
                    set_timeout(
                        move || test_result.set(None),
                        std::time::Duration::from_secs(5),
                    );
                }
            }
            testing.set(false);
        });
    };

    // ── Test Firecrawl (shared) handler ───────────────────────────────────────
    let on_fc_test = move |_| {
        fc_testing.set(true);
        fc_test_result.set(None);

        spawn_local(async move {
            let cfg = fetch_config.get();
            let entry = cfg
                .backends
                .iter()
                .find(|b| b.name == "firecrawl")
                .cloned()
                .unwrap_or_else(|| FetchBackendEntry {
                    name: "firecrawl".to_string(),
                    provider_type: "firecrawl".to_string(),
                    base_url: None,
                    timeout_seconds: None,
                    api_key: None,
                    has_api_key: false,
                    verified: false,
                    shares_search: true,
                });
            match FetchConfigApi::test_connection(&state, &entry).await {
                Ok(result) => {
                    let success = result.success;
                    let msg = result.message;
                    if success {
                        if let Ok(refreshed) = FetchConfigApi::get(&state).await {
                            fetch_config.set(refreshed);
                        }
                    }
                    fc_test_result.set(Some((success, msg)));
                    set_timeout(
                        move || fc_test_result.set(None),
                        std::time::Duration::from_secs(5),
                    );
                }
                Err(e) => {
                    fc_test_result.set(Some((false, format!("Test failed: {e}"))));
                    set_timeout(
                        move || fc_test_result.set(None),
                        std::time::Duration::from_secs(5),
                    );
                }
            }
            fc_testing.set(false);
        });
    };

    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Fetch 供应商"
            </h2>
            <p class="text-xs text-text-tertiary mb-3">
                "URL → Markdown 抓取后端，供 web_fetch 工具使用。"
            </p>

            // ── Section header: master toggle + default-provider selector ─────
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4 mb-4">
                <label class="flex items-center gap-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || form_enabled.get()
                        on:change=on_toggle_enabled
                        class="w-4 h-4 rounded"
                    />
                    <div>
                        <span class="text-sm text-text-primary">"启用 Fetch 供应商"</span>
                        <p class="text-xs text-text-tertiary">
                            "开启后 web_fetch 优先使用所选默认供应商，失败时自动回退其它已配置供应商，再回退内置抓取"
                        </p>
                    </div>
                </label>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "默认供应商"
                    </label>
                    <div class="space-y-2">
                        <label class="flex items-center gap-2 cursor-pointer">
                            <input
                                type="radio"
                                name="fetch_default"
                                prop:checked=move || form_default_provider.get() == "crawl4ai"
                                on:change=on_select_crawl4ai
                                class="w-4 h-4"
                            />
                            <span class="text-sm text-text-primary">"crawl4ai"</span>
                        </label>
                        {move || {
                            let fc_available = fetch_config
                                .get()
                                .backends
                                .iter()
                                .find(|b| b.name == "firecrawl")
                                .is_some_and(|b| b.shares_search && b.has_api_key);
                            view! {
                                <label class="flex items-center gap-2 cursor-pointer">
                                    <input
                                        type="radio"
                                        name="fetch_default"
                                        prop:checked=move || form_default_provider.get() == "firecrawl"
                                        prop:disabled=!fc_available
                                        on:change=on_select_firecrawl
                                        class="w-4 h-4"
                                    />
                                    <span class=move || {
                                        if fc_available {
                                            "text-sm text-text-primary"
                                        } else {
                                            "text-sm text-text-tertiary"
                                        }
                                    }>
                                        "Firecrawl"
                                    </span>
                                    {(!fc_available)
                                        .then(|| {
                                            view! {
                                                <span class="text-xs text-text-tertiary">
                                                    "（请先在 Search 里配置 Firecrawl）"
                                                </span>
                                            }
                                        })}
                                </label>
                            }
                        }}
                    </div>
                </div>
            </div>

            // ── crawl4ai card ─────────────────────────────────────────────────
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4 mb-4">
                // Provider header
                <div class="flex items-center gap-3">
                    <div
                        class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                        style="background-color: #2563EB"
                    >
                        "C"
                    </div>
                    <div class="flex-1">
                        <div class="text-sm font-semibold text-text-primary">"crawl4ai"</div>
                        <div class="text-xs text-text-tertiary">"Self-hosted URL → Markdown scraper"</div>
                    </div>
                    // Verified badge (reactive: re-evaluates after Test/Save)
                    {move || {
                        let verified = fetch_config
                            .get()
                            .backends
                            .iter()
                            .find(|b| b.name == "crawl4ai")
                            .is_some_and(|b| b.verified);
                        if verified {
                            view! {
                                <span class="px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded">
                                    "✓ 已验证"
                                </span>
                            }
                            .into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }
                    }}
                </div>

                // Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Base URL"
                    </label>
                    <input
                        type="text"
                        prop:value=move || form_base_url.get()
                        on:input=move |ev| form_base_url.set(event_target_value(&ev))
                        placeholder="http://localhost:11235"
                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        "crawl4ai 实例地址，如 http://10.0.0.1:11235"
                    </p>
                </div>

                // API Key (write-only via ProviderKeyField; shows 已保存 when has_api_key)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "API Key"
                    </label>
                    <ProviderKeyField
                        value=form_api_key
                        has_api_key=form_has_api_key.into()
                        hint="可选 — 若 crawl4ai 实例开启了认证".to_string()
                    />
                </div>

                // Timeout (seconds)
                <div>
                    <div class="flex items-center justify-between mb-1">
                        <label class="text-sm font-medium text-text-secondary">"超时（秒）"</label>
                        <span class="text-sm text-text-primary font-mono">
                            {move || form_timeout.get()} "s"
                        </span>
                    </div>
                    <input
                        type="number"
                        min="5"
                        max="300"
                        prop:value=move || form_timeout.get().to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                form_timeout.set(v);
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                    />
                </div>

                // Save error
                {move || {
                    save_error.get().map(|e| {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                {e}
                            </div>
                        }
                    })
                }}

                // Save success banner
                {move || {
                    save_success.get().then(|| {
                        view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                                <svg
                                    class="w-4 h-4"
                                    fill="none"
                                    stroke="currentColor"
                                    viewBox="0 0 24 24"
                                >
                                    <path
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M5 13l4 4L19 7"
                                    />
                                </svg>
                                "保存成功"
                            </div>
                        }
                    })
                }}

                // Test result banner
                {move || {
                    test_result.get().map(|(success, msg)| {
                        if success {
                            view! {
                                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                                    <svg
                                        class="w-4 h-4"
                                        fill="none"
                                        stroke="currentColor"
                                        viewBox="0 0 24 24"
                                    >
                                        <path
                                            stroke-linecap="round"
                                            stroke-linejoin="round"
                                            stroke-width="2"
                                            d="M5 13l4 4L19 7"
                                        />
                                    </svg>
                                    {msg}
                                </div>
                            }
                            .into_any()
                        } else {
                            view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm flex items-center gap-2">
                                    <svg
                                        class="w-4 h-4"
                                        fill="none"
                                        stroke="currentColor"
                                        viewBox="0 0 24 24"
                                    >
                                        <path
                                            stroke-linecap="round"
                                            stroke-linejoin="round"
                                            stroke-width="2"
                                            d="M6 18L18 6M6 6l12 12"
                                        />
                                    </svg>
                                    {msg}
                                </div>
                            }
                            .into_any()
                        }
                    })
                }}

                // Test + Save buttons
                <div class="flex gap-3">
                    <button
                        on:click=on_test
                        prop:disabled=move || testing.get()
                        class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium text-sm"
                    >
                        {move || if testing.get() { "测试中…" } else { "测试连接" }}
                    </button>
                    <button
                        on:click=on_save
                        prop:disabled=move || saving.get()
                        class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                    >
                        {move || if saving.get() { "保存中…" } else { "保存" }}
                    </button>
                </div>
            </div>

            // ── Firecrawl (shared) row ────────────────────────────────────────
            // Reactive: re-evaluates whenever fetch_config changes (after save/test).
            {move || {
                let cfg = fetch_config.get();
                let fc_backend = cfg.backends.iter().find(|b| b.name == "firecrawl").cloned();
                // "Configured" = server reports has_api_key=true (key lives in search:firecrawl vault)
                let fc_configured = fc_backend.as_ref().is_some_and(|b| b.shares_search && b.has_api_key);
                let fc_verified = fc_backend.as_ref().is_some_and(|b| b.verified);

                if fc_configured {
                    view! {
                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                            <div class="flex items-center gap-3">
                                <div
                                    class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                    style="background-color: #FF6B35"
                                >
                                    "F"
                                </div>
                                <div class="flex-1">
                                    <div class="text-sm font-semibold text-text-primary">
                                        "Firecrawl"
                                    </div>
                                    <div class="text-xs text-text-tertiary">
                                        "复用 Search 里的 Firecrawl 配置"
                                    </div>
                                </div>
                                {if fc_verified {
                                    view! {
                                        <span class="px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded">
                                            "✓ 已验证"
                                        </span>
                                    }
                                    .into_any()
                                } else {
                                    view! { <span></span> }.into_any()
                                }}
                            </div>

                            // Firecrawl test result
                            {move || {
                                fc_test_result.get().map(|(success, msg)| {
                                    if success {
                                        view! {
                                            <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                                                {msg}
                                            </div>
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                                {msg}
                                            </div>
                                        }
                                        .into_any()
                                    }
                                })
                            }}

                            <button
                                on:click=on_fc_test
                                prop:disabled=move || fc_testing.get()
                                class="w-full px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium text-sm"
                            >
                                {move || if fc_testing.get() { "测试中…" } else { "测试连接" }}
                            </button>
                        </div>
                    }
                    .into_any()
                } else {
                    // Firecrawl not yet configured in Search — show disabled hint
                    view! {
                        <div class="bg-surface-raised border border-border rounded-xl p-4 opacity-60">
                            <div class="flex items-center gap-3">
                                <div class="w-8 h-8 rounded-lg flex items-center justify-center bg-surface-sunken text-text-tertiary text-sm font-bold shrink-0">
                                    "F"
                                </div>
                                <div class="flex-1">
                                    <div class="text-sm font-semibold text-text-secondary">
                                        "Firecrawl (shared)"
                                    </div>
                                    <div class="text-xs text-text-tertiary">
                                        "请先在上方 Search 里配置 Firecrawl"
                                    </div>
                                </div>
                            </div>
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}
