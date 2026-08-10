//! Add Custom Provider panel — manual endpoint / key entry for non-preset providers.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{GenerationProviderConfig, GenerationProvidersApi};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::i18n::{t, t_string, use_i18n};

use super::extract_base_url;

#[component]
pub(super) fn AddCustomProviderPanel(
    category: GenerationType,
    on_added: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let name = RwSignal::new(String::new());
    // Auto-infer provider_type from category
    let default_provider_type = match category {
        GenerationType::Speech => "openai_tts",
        GenerationType::Transcription => "openai_compat",
        GenerationType::Image => "openai",
        _ => "openai_compat",
    };
    let provider_type = RwSignal::new(default_provider_type.to_string());
    let api_key = RwSignal::new(String::new());
    let base_url = RwSignal::new(String::new());
    let edit_url = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let timeout = RwSignal::new(60u64);

    let (adding, set_adding) = signal(false);
    let (testing, set_testing) = signal(false);
    let (add_error, set_add_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);

    let build_config = move || -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: provider_type.get(),
            api_key: {
                let key = api_key.get();
                if key.is_empty() {
                    None
                } else {
                    Some(key)
                }
            },
            secret_name: None,
            base_url: {
                let url = base_url.get();
                let url = extract_base_url(&url);
                if url.is_empty() {
                    None
                } else {
                    Some(url)
                }
            },
            edit_url: {
                let url = edit_url.get();
                if url.is_empty() {
                    None
                } else {
                    Some(url)
                }
            },
            voices_url: None,
            models: if form_model.get().is_empty() {
                vec![]
            } else {
                form_model
                    .get()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            },
            enabled: true,
            color: "#808080".to_string(),
            capabilities: vec![category],
            timeout_seconds: timeout.get(),
            verified: false,
            defaults: Default::default(),
        }
    };

    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_add_error.set(None);

        let config = build_config();
        let ptype = config.provider_type.clone();
        let key = config.api_key.clone();
        let url = config.base_url.clone();
        let mdl = config.models.first().cloned();

        spawn_local(async move {
            match GenerationProvidersApi::test_connection(&state, &ptype, key, url, mdl, None).await
            {
                Ok(result) => {
                    set_testing.set(false);
                    set_test_result.set(Some((result.success, result.message)));
                }
                Err(e) => {
                    set_testing.set(false);
                    // No frame: the success arm shows `result.message` bare, so
                    // a non-refusal keeps reading exactly as it did. What the
                    // wrapper is here for is the OTHER branch — this family is
                    // admin-gated, and a member was reading the raw protocol
                    // string in the red "test failed" slot.
                    set_test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    )));
                }
            }
        });
    };

    let gen_type_str = category.as_str().to_string();
    let handle_add = move |_| {
        let n = name.get();
        if n.is_empty() {
            set_add_error.set(Some("Provider name is required".to_string()));
            return;
        }
        if provider_type.get().is_empty() {
            set_add_error.set(Some("Provider type is required".to_string()));
            return;
        }

        set_adding.set(true);
        set_add_error.set(None);

        let config = build_config();
        let gt = gen_type_str.clone();

        spawn_local(async move {
            match GenerationProvidersApi::create(&state, &n, config, &gt).await {
                Ok(_) => {
                    set_adding.set(false);
                    on_added();
                }
                Err(e) => {
                    set_adding.set(false);
                    set_add_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to add: {e}")
                        }),
                    ));
                }
            }
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <h2 class="text-xl font-semibold text-text-primary">{t!(i18n, settings.generation.add_custom_title)}</h2>
                    <button
                        on:click=move |_| on_cancel()
                        class="text-text-tertiary hover:text-text-primary transition-colors"
                    >
                        {t!(i18n, common.cancel)}
                    </button>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Form fields
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                // Name
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.provider_name_label)}</label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder=t_string!(i18n, settings.generation.provider_name_placeholder).to_string()
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.provider_name_hint)}</p>
                </div>

                // Provider Type (auto-inferred from capabilities, editable)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.provider_type_label)}</label>
                    <input
                        type="text"
                        value=move || provider_type.get()
                        on:input=move |ev| provider_type.set(event_target_value(&ev))
                        placeholder=t_string!(i18n, settings.generation.provider_type_placeholder).to_string()
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.provider_type_hint)}</p>
                </div>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_key_label)}</label>
                    <ProviderKeyField
                        value=api_key
                        has_api_key=Signal::derive(|| false)
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.model_label)}</label>
                    <input
                        type="text"
                        value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder="e.g. dall-e-3, stable-diffusion-xl"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.model_hint)}</p>
                </div>

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Base URL"</label>
                    <input
                        type="text"
                        value=move || base_url.get()
                        on:input=move |ev| base_url.set(event_target_value(&ev))
                        placeholder="https://api.openai.com"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.api_base_url_hint)}</p>
                </div>

                // Edit Endpoint URL (optional)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.edit_endpoint_label)}</label>
                    <input
                        type="text"
                        value=move || edit_url.get()
                        on:input=move |ev| edit_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/edits"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.edit_endpoint_hint)}</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.generation.timeout_label)} ": " {move || timeout.get()} "s"
                    </label>
                    <input
                        type="range" min="10" max="300" step="10"
                        value=move || timeout.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() { timeout.set(v); }
                        }
                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                </div>
            </div>

            // Category indicator (read-only, determined by current tab)
            <div class="bg-surface-raised border border-border rounded-xl p-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-2">"CATEGORY"</h3>
                <div class="flex items-center gap-2 text-sm text-text-primary">
                    <span>{category.icon()}</span>
                    <span>{category.display_name()}</span>
                </div>
            </div>

            // Test result
            {move || {
                if let Some((success, message)) = test_result.get() {
                    if success {
                        view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded">
                                <p class="text-sm text-success">{message}</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded">
                                <p class="text-sm text-danger">{message}</p>
                            </div>
                        }.into_any()
                    }
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Error
            {move || add_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{e}</div>
            })}

            // Actions
            <div class="flex flex-row gap-3 pt-2">
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if testing.get() { t_string!(i18n, settings.generation.testing).to_string() } else { t_string!(i18n, settings.generation.test_connection).to_string() }}
                </button>

                <button
                    on:click=handle_add
                    disabled=move || adding.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if adding.get() { t_string!(i18n, settings.generation.adding).to_string() } else { t_string!(i18n, settings.generation.add_provider).to_string() }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}
