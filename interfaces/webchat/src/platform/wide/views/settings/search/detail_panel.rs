use super::presentation::{find_backend, find_preset};
use crate::api::{SearchBackendEntry, SearchConfig, SearchConfigApi};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Detail Panel (Right Side)
// ============================================================================

#[component]
pub(super) fn ProviderDetailPanel(
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
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Test failed: {e}")
                        }),
                    ));
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
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to save: {e}")
                        }),
                    ));
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
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to save: {e}")
                        }),
                    ));
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
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Delete failed: {e}"),
                    )));
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
                                        {preset.map(|p| p.description.to_string()).unwrap_or_else(|| t_string!(i18n, settings.search.provider_generic_desc).to_string())}
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
                                                        placeholder=t_string!(i18n, settings.search.google_cse_placeholder)
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
                                                        {t!(i18n, settings.search.engines)}
                                                    </label>
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_engines.get()
                                                        on:input=move |ev| form_engines.set(event_target_value(&ev))
                                                        placeholder=t_string!(i18n, settings.search.engines_placeholder)
                                                        class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                                    />
                                                    <p class="mt-1 text-xs text-text-tertiary">
                                                        {t!(i18n, settings.search.engines_hint)}
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
                                    {t!(i18n, settings.search.saved_successfully)}
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
                                            {t!(i18n, settings.search.connection_successful)}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm flex items-center gap-2">
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                            </svg>
                                            {t!(i18n, settings.search.connection_failed)}
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

