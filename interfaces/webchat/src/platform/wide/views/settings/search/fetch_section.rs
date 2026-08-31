use crate::api::{FetchBackendEntry, FetchConfig, FetchConfigApi};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Fetch Providers Section
// ============================================================================

/// Self-contained settings section for URL→Markdown fetch backends.
///
/// Renders below the search providers in the left panel (not entangled with the
/// search master-detail). Manages its own FetchConfig signal and form state.
#[component]
pub(super) fn FetchProvidersSection() -> impl IntoView {
    let i18n = use_i18n();
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
                    save_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to save: {e}")
                        }),
                    ));
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
                    test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Test failed: {e}")
                        }),
                    )));
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
                    fc_test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Test failed: {e}")
                        }),
                    )));
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
                {t!(i18n, settings.fetch.title)}
            </h2>
            <p class="text-xs text-text-tertiary mb-3">
                {t!(i18n, settings.fetch.description)}
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
                        <span class="text-sm text-text-primary">{t!(i18n, settings.fetch.enable)}</span>
                        <p class="text-xs text-text-tertiary">
                            {t!(i18n, settings.fetch.enable_desc)}
                        </p>
                    </div>
                </label>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        {t!(i18n, settings.fetch.default_provider)}
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
                                                    {t!(i18n, settings.fetch.configure_firecrawl_first_inline)}
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
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.search.crawl4ai_desc)}</div>
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
                                    {t!(i18n, settings.fetch.verified)}
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
                        {t!(i18n, settings.search.base_url)}
                    </label>
                    <input
                        type="text"
                        prop:value=move || form_base_url.get()
                        on:input=move |ev| form_base_url.set(event_target_value(&ev))
                        placeholder="http://localhost:11235"
                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        {t!(i18n, settings.fetch.crawl4ai_url_hint)}
                    </p>
                </div>

                // API Key (write-only via ProviderKeyField; shows saved when has_api_key)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.search.api_key)}
                    </label>
                    <ProviderKeyField
                        value=form_api_key
                        has_api_key=form_has_api_key.into()
                        hint=t_string!(i18n, settings.fetch.crawl4ai_token_hint).to_string()
                    />
                </div>

                // Timeout (seconds)
                <div>
                    <div class="flex items-center justify-between mb-1">
                        <label class="text-sm font-medium text-text-secondary">
                            {t!(i18n, settings.fetch.timeout_seconds)}
                        </label>
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
                                {t!(i18n, settings.fetch.saved)}
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
                        {move || {
                            if testing.get() {
                                t_string!(i18n, settings.search.testing).to_string()
                            } else {
                                t_string!(i18n, settings.search.test_connection).to_string()
                            }
                        }}
                    </button>
                    <button
                        on:click=on_save
                        prop:disabled=move || saving.get()
                        class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                    >
                        {move || {
                            if saving.get() {
                                t_string!(i18n, common.saving).to_string()
                            } else {
                                t_string!(i18n, common.save_short).to_string()
                            }
                        }}
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
                                        {t!(i18n, settings.fetch.reuse_firecrawl)}
                                    </div>
                                </div>
                                {if fc_verified {
                                    view! {
                                        <span class="px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded">
                                            {t!(i18n, settings.fetch.verified)}
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
                                {move || {
                                    if fc_testing.get() {
                                        t_string!(i18n, settings.search.testing).to_string()
                                    } else {
                                        t_string!(i18n, settings.search.test_connection).to_string()
                                    }
                                }}
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
                                        {t!(i18n, settings.search.firecrawl_shared)}
                                    </div>
                                    <div class="text-xs text-text-tertiary">
                                        {t!(i18n, settings.fetch.configure_firecrawl_first)}
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
