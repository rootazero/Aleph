//! Right-pane editor for a selected preset reranker. Each preset shares the same
//! form layout (API key, model, base URL, timeout, weight). The stored secret is
//! never echoed back: the key field starts empty and `has_api_key` (reported by
//! `RerankConfigApi::get_for_provider`) only drives a status hint. After a
//! successful test/save the config is refetched to surface `verified`.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{RerankConfig, RerankConfigApi, RerankProviderType};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

use super::RERANK_PRESETS;

#[component]
pub(super) fn ProviderDetailPanel(
    provider_key: String,
    config: RwSignal<Option<RerankConfig>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let preset = RERANK_PRESETS.iter().find(|p| p.key == provider_key);
    let preset_name = preset.map_or_else(
        || t_string!(i18n, settings.reranking.preset_custom).to_string(),
        |p| p.name.to_string(),
    );
    let preset_key = provider_key.clone();
    let provider_key_for_badge = provider_key.clone();
    let provider_key_for_test = provider_key.clone();
    let provider_key_for_save = provider_key.clone();
    let default_api_base = preset.map(|p| p.default_api_base).unwrap_or("").to_string();

    // Initialize form state from config
    let rerank_cfg = config.get().unwrap_or_default();
    let is_current_provider = rerank_cfg.provider.as_str() == provider_key;

    let enabled = RwSignal::new(if is_current_provider {
        rerank_cfg.enabled
    } else {
        true
    });
    let api_base = RwSignal::new(if is_current_provider {
        rerank_cfg.api_base.clone()
    } else {
        String::new()
    });
    let api_key = RwSignal::new(String::new());
    // Whether THIS provider already has a key in the vault. The secret is never
    // echoed; the field starts empty and an empty value on save keeps the key.
    let provider_has_key = RwSignal::new(false);

    // Fetch this provider's key presence from the vault (no secret is returned).
    {
        let pk = provider_key.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            if let Ok(cfg) = RerankConfigApi::get_for_provider(&state, &pk).await {
                provider_has_key.set(cfg.has_api_key);
            }
        });
    }
    let form_model = RwSignal::new({
        if is_current_provider && !rerank_cfg.model.is_empty() {
            rerank_cfg.model.clone()
        } else {
            preset
                .map(|p| p.default_model.to_string())
                .unwrap_or_default()
        }
    });
    let timeout_ms = RwSignal::new(if is_current_provider {
        rerank_cfg.timeout_ms
    } else {
        5000
    });
    let rerank_weight = RwSignal::new(if is_current_provider {
        rerank_cfg.rerank_weight
    } else {
        0.6
    });

    // Action states
    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (save_success, set_save_success) = signal(false);
    let (action_error, set_action_error) = signal(Option::<String>::None);

    // Build RerankConfig from form state
    let preset_key_for_build = preset_key;
    let build_rerank_config = move || -> RerankConfig {
        RerankConfig {
            enabled: enabled.get(),
            provider: RerankProviderType::from_str_val(&preset_key_for_build),
            api_base: api_base.get(),
            api_key: api_key.get(),
            model: form_model.get(),
            timeout_ms: timeout_ms.get(),
            rerank_weight: rerank_weight.get(),
            // verified is server-tracked; has_api_key is get-only. Both are
            // ignored by the update handler — sent as defaults from the form.
            verified: false,
            has_api_key: false,
        }
    };

    // Grab state once for closures
    let state = expect_context::<DashboardState>();

    // Test connection
    let build_for_test = build_rerank_config.clone();
    let test_state = state;
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let rerank = build_for_test();
        let state = test_state;
        let pk_test = provider_key_for_test.clone();
        spawn_local(async move {
            match RerankConfigApi::test(&state, rerank).await {
                Ok(resp) => {
                    if resp.success {
                        set_test_result.set(Some((
                            true,
                            format!(
                                "Success! {} results, top score: {:.3}",
                                resp.results_count, resp.top_score
                            ),
                        )));
                        // Refetch to pick up verified=true and key presence.
                        if let Ok(cfg) = RerankConfigApi::get_for_provider(&state, &pk_test).await {
                            provider_has_key.set(cfg.has_api_key);
                            config.set(Some(cfg));
                        }
                    } else {
                        set_test_result.set(Some((
                            false,
                            format!(
                                "Failed: {}",
                                resp.error.unwrap_or_else(|| "Unknown error".to_string())
                            ),
                        )));
                    }
                }
                Err(e) => {
                    set_test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("RPC error: {e}")
                        }),
                    )));
                }
            }
            set_testing.set(false);
        });
    };

    // Save handler
    let build_for_save = build_rerank_config;
    let save_state = state;
    let handle_save = move |_| {
        set_action_error.set(None);
        set_save_success.set(false);
        set_saving.set(true);

        let rerank = build_for_save();
        let state = save_state;
        let pk_save = provider_key_for_save.clone();
        spawn_local(async move {
            match RerankConfigApi::update(&state, rerank).await {
                Ok(_) => {
                    // Key now lives in the vault — clear the input so it shows the
                    // "configured" hint, and refetch to surface verified + presence.
                    api_key.set(String::new());
                    if let Ok(cfg) = RerankConfigApi::get_for_provider(&state, &pk_save).await {
                        provider_has_key.set(cfg.has_api_key);
                        config.set(Some(cfg));
                    }
                    set_save_success.set(true);
                    set_timeout(
                        move || set_save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Save failed: {e}")
                        }),
                    ));
                }
            }
            set_saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {preset_name}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {t!(i18n, settings.reranking.cross_encoder_desc)}
                        </p>
                    </div>
                    <div class="flex gap-1">
                        {move || {
                            let cfg = config.get().unwrap_or_default();
                            let is_cur = cfg.provider.as_str() == provider_key_for_badge;
                            view! {
                                <ProviderBadges state=BadgeState {
                                    is_default: is_cur && cfg.enabled,
                                    verified: is_cur && cfg.verified,
                                } />
                            }
                        }}
                    </div>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.reranking.configuration)}</h3>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.reranking.api_key)}
                    </label>
                    <ProviderKeyField
                        value=api_key
                        has_api_key=provider_has_key.into()
                        hint=t_string!(i18n, settings.reranking.api_key_placeholder).to_string()
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.reranking.model)}
                    </label>
                    <input
                        type="text"
                        value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder=move || t_string!(i18n, settings.reranking.model_placeholder).to_string()
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.reranking.model_hint)}</p>
                </div>

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.reranking.base_url)}
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder=default_api_base
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Enabled
                <label class="flex items-center gap-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || enabled.get()
                        on:change=move |ev| enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <div>
                        <span class="text-sm text-text-primary">{t!(i18n, settings.reranking.enabled)}</span>
                        <p class="text-xs text-text-tertiary">{t!(i18n, settings.reranking.enabled_desc)}</p>
                    </div>
                </label>
            </div>

            // Parameters card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.reranking.parameters)}</h3>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            {t!(i18n, settings.reranking.timeout_ms)}
                        </label>
                        <input
                            type="number"
                            min="100"
                            value=move || timeout_ms.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                    timeout_ms.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            {t!(i18n, settings.reranking.rerank_weight)}
                        </label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            value=move || rerank_weight.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                    rerank_weight.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.reranking.rerank_weight_hint)}</p>
                    </div>
                </div>
            </div>

            // Test result
            {move || {
                if let Some((success, message)) = test_result.get() {
                    if success {
                        view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded-lg">
                                <p class="text-sm text-success">{message}</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg">
                                <p class="text-sm text-danger">{message}</p>
                            </div>
                        }.into_any()
                    }
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Save success
            {move || save_success.get().then(|| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{t!(i18n, settings.reranking.saved)}</div>
            })}

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Actions
            <div class="flex flex-row gap-3 pt-2">
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if testing.get() { t_string!(i18n, settings.reranking.testing).to_string() } else { t_string!(i18n, settings.reranking.test_connection).to_string() }}
                </button>

                <button
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}
