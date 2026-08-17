//! Add-embedding-provider panel — the form behind both "+ Add Custom Provider"
//! and picking an unconfigured preset row.
//!
//! Without `prefill` it creates a `preset: "custom"` config from scratch; with
//! one it starts from that preset's id / endpoint / model / dimensions and
//! keeps its `preset` label so the server's `apply_embedding_preset_defaults`
//! still recognises the row. ID + Name + API base + model are required either
//! way.
//!
//! # Why a preset row opens this instead of writing straight through
//!
//! Clicking an unconfigured preset card used to `POST embedding_providers.add`
//! on the spot, so there was no way to look at a preset without leaving a
//! keyless provider behind in the config. Nothing downstream broke — `add`
//! does not touch `active_provider_id`, and `remove` refuses to delete the
//! active row — but browsing should not write. Now nothing is persisted until
//! Save, which is what the chat and generation preset forms have always done.

use crate::api::{EmbeddingProviderConfig, EmbeddingProvidersApi};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub(super) fn AddProviderPanel(
    /// Receives the id of the row just written, so the caller can select it —
    /// the operator still has to make it active, and that control lives in the
    /// detail pane.
    on_added: impl Fn(String) + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
    /// Starting values, when the operator picked a preset row rather than
    /// "+ Add Custom Provider". Taken by value: the caller re-renders this
    /// panel when it changes, which is also what resets a half-filled form
    /// between two different presets.
    #[prop(optional_no_strip)]
    prefill: Option<EmbeddingProviderConfig>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state. The preset label is *not* a form field — it selects the
    // server-side defaults for this row and has no meaning the operator could
    // edit — so it rides along in a `StoredValue` instead.
    let preset = StoredValue::new(
        prefill
            .as_ref()
            .map_or_else(|| "custom".to_string(), |p| p.preset.clone()),
    );
    let from_preset = prefill.is_some();
    let id = RwSignal::new(prefill.as_ref().map(|p| p.id.clone()).unwrap_or_default());
    let name = RwSignal::new(prefill.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let api_base = RwSignal::new(
        prefill
            .as_ref()
            .map(|p| p.api_base.clone())
            .unwrap_or_default(),
    );
    let api_key = RwSignal::new(String::new());
    let form_model = RwSignal::new(
        prefill
            .as_ref()
            .map(|p| p.model.clone())
            .unwrap_or_default(),
    );
    let dimensions = RwSignal::new(prefill.as_ref().map_or(1536, |p| p.dimensions));

    let (adding, set_adding) = signal(false);
    let (testing, set_testing) = signal(false);
    let (add_error, set_add_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);

    // Build config from form
    let build_config = move || -> EmbeddingProviderConfig {
        EmbeddingProviderConfig {
            id: id.get(),
            name: name.get(),
            preset: preset.get_value(),
            api_base: api_base.get(),
            api_key_env: None,
            api_key: {
                let key = api_key.get();
                if key.is_empty() {
                    None
                } else {
                    Some(key)
                }
            },
            model: form_model.get(),
            dimensions: dimensions.get(),
            batch_size: 32,
            timeout_ms: 10000,
            enabled: true,
        }
    };

    // Test handler
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_add_error.set(None);

        let config = build_config();

        spawn_local(async move {
            match EmbeddingProvidersApi::test(&state, None, config).await {
                Ok(result) => {
                    set_testing.set(false);
                    set_test_result.set(Some((result.success, result.message)));
                }
                Err(e) => {
                    set_testing.set(false);
                    // Unframed on purpose — see `generation_providers/
                    // add_custom.rs`'s twin.
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

    // Add handler
    let handle_add = move |_| {
        set_adding.set(true);
        set_add_error.set(None);

        let config = build_config();
        let id = config.id.clone();

        if id.is_empty() || config.name.is_empty() {
            set_add_error.set(Some("ID and Name are required".to_string()));
            set_adding.set(false);
            return;
        }

        // Read before the move: the operator may have edited the id away from
        // whatever the preset suggested, and the row that now exists is the one
        // the caller has to select.
        let new_id = config.id.clone();
        spawn_local(async move {
            match EmbeddingProvidersApi::add(&state, config).await {
                Ok(_) => {
                    set_adding.set(false);
                    on_added(new_id);
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
                    <h2 class="text-xl font-semibold text-text-primary">
                        {move || if from_preset {
                            view! { {t!(i18n, settings.embedding.add_provider)} }.into_any()
                        } else {
                            view! { {t!(i18n, settings.embedding.add_custom_provider)} }.into_any()
                        }}
                    </h2>
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
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.embedding.configuration)}</h3>
                // ID
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.provider_id)}</label>
                    <input
                        type="text"
                        value=move || id.get()
                        on:input=move |ev| id.set(event_target_value(&ev))
                        placeholder="e.g., my-openai"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.embedding.provider_id_hint)}</p>
                </div>

                // Name
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.display_name)}</label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder="e.g., My OpenAI Embedding"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.api_key)}</label>
                    <ProviderKeyField
                        value=api_key
                        has_api_key=Signal::derive(|| false)
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.model)}</label>
                    <input
                        type="text"
                        value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder="e.g. text-embedding-3-small"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.embedding.model_hint)}</p>
                </div>

                // Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.base_url)}</label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder="https://api.openai.com/v1"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Dimensions
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.embedding.dimensions)}</label>
                    <input
                        type="number"
                        value=move || dimensions.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                                dimensions.set(v);
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.embedding.dimensions_hint)}</p>
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
                    {move || if testing.get() { t_string!(i18n, settings.embedding.testing).to_string() } else { t_string!(i18n, settings.embedding.test_connection).to_string() }}
                </button>

                <button
                    on:click=handle_add
                    disabled=move || adding.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if adding.get() { t_string!(i18n, settings.embedding.adding).to_string() } else { t_string!(i18n, settings.embedding.add_provider).to_string() }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}
