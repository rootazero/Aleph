//! Add-custom-reranker panel. Saves as a `RerankProviderType::Vllm` provider
//! with a user-supplied endpoint; API base + model are required.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{RerankConfig, RerankConfigApi, RerankProviderType};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub(super) fn AddCustomProviderPanel(
    config: RwSignal<Option<RerankConfig>>,
    on_saved: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let i18n = use_i18n();
    // Form state
    let name = RwSignal::new(String::new());
    let api_base = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let timeout_ms = RwSignal::new(5000u64);
    let rerank_weight = RwSignal::new(0.6f32);

    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (action_error, set_action_error) = signal(Option::<String>::None);

    // Build RerankConfig from form state
    let build_rerank_config = move || -> RerankConfig {
        RerankConfig {
            enabled: true,
            provider: RerankProviderType::Vllm, // Custom providers use vLLM-compatible API
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
    let build_for_test = build_rerank_config;
    let test_state = state;
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let rerank = build_for_test();
        let state = test_state;
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

    // Save handler — saves as vLLM provider with custom endpoint
    let build_for_save = build_rerank_config;
    let save_state = state;
    let handle_save = move |_| {
        // Validate
        if api_base.get().is_empty() {
            set_action_error.set(Some(
                "API Base URL is required for custom providers".to_string(),
            ));
            return;
        }
        if form_model.get().is_empty() {
            set_action_error.set(Some("Model name is required".to_string()));
            return;
        }

        set_saving.set(true);
        set_action_error.set(None);

        let rerank = build_for_save();
        let rerank_clone = rerank;
        let state = save_state;
        spawn_local(async move {
            match RerankConfigApi::update(&state, rerank_clone.clone()).await {
                Ok(_) => {
                    // Clear api_key from local signal (key lives in vault, not in memory)
                    let mut saved = rerank_clone;
                    saved.api_key = String::new();
                    config.set(Some(saved));
                    set_saving.set(false);
                    on_saved();
                }
                Err(e) => {
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Save failed: {e}")
                        }),
                    ));
                    set_saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {t!(i18n, settings.reranking.add_custom)}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {t!(i18n, settings.reranking.custom_endpoint_desc)}
                        </p>
                    </div>
                    <button
                        on:click=move |_| on_cancel()
                        class="px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary border border-border rounded-lg hover:bg-surface-raised transition-colors"
                    >
                        {t!(i18n, settings.reranking.cancel)}
                    </button>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Provider name (optional display name)
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.reranking.provider_info)}</h3>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.reranking.name)}
                    </label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder=move || t_string!(i18n, settings.reranking.name_placeholder).to_string()
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
            </div>

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
                        has_api_key=Signal::derive(|| false)
                        hint=t_string!(i18n, settings.reranking.api_key_optional_placeholder).to_string()
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
                        <span class="text-danger ml-1">{t!(i18n, settings.reranking.base_url_required)}</span>
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder=move || t_string!(i18n, settings.reranking.base_url_custom_placeholder).to_string()
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
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
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, settings.reranking.add_provider).to_string() }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}
