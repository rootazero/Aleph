//! Right-pane editor for a selected embedding provider — edits `EmbeddingProviderConfig`,
//! handles Test / Save / Activate / Delete, and embeds the [`super::reembed_card`] for
//! the (Provider-agnostic) reembed migration UI.

use crate::api::{EmbeddingProviderConfig, EmbeddingProviderEntry, EmbeddingProvidersApi};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::reembed_card::ReembedMigrationCard;

#[component]
pub(super) fn ProviderDetailPanel(
    provider: EmbeddingProviderEntry,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let provider_id = provider.id.clone();
    let is_active = provider.is_active;
    let is_custom = provider.preset == "custom";
    let provider_has_api_key = provider.has_api_key;
    let provider_verified = provider.verified;

    // Clone fields needed in multiple closures and the view
    let provider_name = provider.name.clone();
    let provider_preset = provider.preset.clone();
    let provider_api_key_env = provider.api_key_env.clone();
    let provider_batch_size = provider.batch_size;
    let provider_timeout_ms = provider.timeout_ms;

    // Editable fields
    let api_base = RwSignal::new(provider.api_base.clone());
    let api_key = RwSignal::new(String::new());
    let form_model = RwSignal::new(provider.model.clone());
    let dimensions = RwSignal::new(provider.dimensions);
    let enabled = RwSignal::new(provider.enabled);

    // Action states
    let (deleting, set_deleting) = signal(false);
    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (activating, set_activating) = signal(false);
    let (action_error, set_action_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (save_success, set_save_success) = signal(false);

    // Build config from current field values (captured clones, not provider directly)
    let build_config = {
        let pid = provider_id.clone();
        let pname = provider_name.clone();
        let ppreset = provider_preset.clone();
        let pkey_env = provider_api_key_env.clone();
        move || -> EmbeddingProviderConfig {
            EmbeddingProviderConfig {
                id: pid.clone(),
                name: pname.clone(),
                preset: ppreset.clone(),
                api_base: api_base.get(),
                api_key_env: pkey_env.clone(),
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
                batch_size: provider_batch_size,
                timeout_ms: provider_timeout_ms,
                enabled: enabled.get(),
            }
        }
    };

    // Test connection handler
    let build_config_for_test = build_config.clone();
    let provider_id_for_test = provider_id.clone();
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let config = build_config_for_test();
        let id = provider_id_for_test.clone();

        spawn_local(async move {
            match EmbeddingProvidersApi::test(&state, Some(&id), config).await {
                Ok(result) => {
                    set_testing.set(false);
                    set_test_result.set(Some((result.success, result.message)));
                    if result.success {
                        on_reload();
                    }
                }
                Err(e) => {
                    set_testing.set(false);
                    set_test_result.set(Some((false, e)));
                }
            }
        });
    };

    // Save handler
    let build_config_for_save = build_config;
    let handle_save = move |_| {
        set_saving.set(true);
        set_action_error.set(None);
        set_save_success.set(false);

        let config = build_config_for_save();
        let id = config.id.clone();

        spawn_local(async move {
            match EmbeddingProvidersApi::update(&state, &id, config).await {
                Ok(_) => {
                    set_saving.set(false);
                    set_save_success.set(true);
                    on_reload();
                    set_timeout(
                        move || set_save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    set_saving.set(false);
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Save failed: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Set active handler
    let provider_id_for_activate = provider_id.clone();
    let handle_activate = move |_| {
        let id = provider_id_for_activate.clone();
        set_activating.set(true);
        set_action_error.set(None);

        spawn_local(async move {
            match EmbeddingProvidersApi::set_active(&state, &id).await {
                Ok(()) => {
                    set_activating.set(false);
                    on_reload();
                }
                Err(e) => {
                    set_activating.set(false);
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Activation failed: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Delete handler
    let provider_id_for_delete = provider_id.clone();
    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
        let id = provider_id_for_delete.clone();
        set_deleting.set(true);
        set_action_error.set(None);

        spawn_local(async move {
            match EmbeddingProvidersApi::remove(&state, &id).await {
                Ok(_) => {
                    set_deleting.set(false);
                    on_reload();
                }
                Err(e) => {
                    set_deleting.set(false);
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Delete failed: {e}")
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
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {provider_name}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {format!("ID: {provider_id}")}
                        </p>
                    </div>
                    <div class="flex gap-1">
                        <ProviderBadges state=BadgeState { is_default: is_active, verified: provider_verified } />
                    </div>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.embedding.configuration)}</h3>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.embedding.api_key)}
                    </label>
                    <ProviderKeyField
                        value=api_key
                        has_api_key=Signal::derive(move || provider_has_api_key)
                    />
                    {provider_api_key_env.map(|env_var| view! {
                        <p class="mt-1 text-xs text-text-tertiary">
                            {format!("Env var: {env_var}")}
                        </p>
                    })}
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.embedding.model)}
                    </label>
                    <input
                        type="text"
                        value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder="e.g. text-embedding-3-small"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.embedding.model_hint)}</p>
                </div>

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.embedding.base_url)}
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder={
                            let default_base = match provider_preset.as_str() {
                                "silicon_flow" => "https://api.siliconflow.cn/v1",
                                "open_ai" => "https://api.openai.com/v1",
                                "ollama" => "http://localhost:11434/v1",
                                "jina" => "https://api.jina.ai/v1",
                                "mistral" => "https://api.mistral.ai/v1",
                                _ => "https://api.example.com/v1",
                            };
                            default_base
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Dimensions
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.embedding.dimensions)}
                    </label>
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
                        <span class="text-sm text-text-primary">{t!(i18n, settings.embedding.enabled)}</span>
                        <p class="text-xs text-text-tertiary">{t!(i18n, settings.embedding.enabled_desc)}</p>
                    </div>
                </label>
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
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">"Saved"</div>
            })}

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Actions — Row 1: Test + Save
            <div class="flex flex-row gap-3 pt-2">
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if testing.get() { t_string!(i18n, settings.embedding.testing).to_string() } else { t_string!(i18n, settings.embedding.test_connection).to_string() }}
                </button>

                <button
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>

            // Actions — Row 2: Set as Default + Delete (only for existing providers)
            {if !is_active || is_custom {
                Some(view! {
                    <div class="flex flex-row gap-3">
                        {if !is_active {
                            Some(view! {
                                <div class="flex-1 flex flex-col gap-1">
                                    <button
                                        on:click=handle_activate
                                        disabled=move || activating.get() || !provider_verified
                                        class="w-full px-4 py-2.5 bg-success-subtle border border-success/20 text-success rounded-lg hover:bg-success-subtle/80 disabled:opacity-50 transition-colors font-medium"
                                    >
                                        {move || if activating.get() { t_string!(i18n, settings.embedding.setting_default).to_string() } else { t_string!(i18n, settings.embedding.set_as_default).to_string() }}
                                    </button>
                                    {(!provider_verified).then(|| view! {
                                        <p class="text-xs text-text-tertiary">{t!(i18n, settings.embedding.verify_before_default)}</p>
                                    })}
                                </div>
                            })
                        } else {
                            None
                        }}
                        {if is_custom {
                            Some(view! {
                                {move || if confirming.get() {
                                    view! {
                                        <ConfirmButton confirming=confirming on_confirm=on_confirm_delete.clone() width_class="flex-1" />
                                    }.into_any()
                                } else {
                                    view! {
                                        <button
                                            on:click=move |_| confirming.set(true)
                                            disabled=move || deleting.get()
                                            class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50 transition-colors font-medium"
                                        >
                                            {move || if deleting.get() { t_string!(i18n, settings.embedding.deleting).to_string() } else { t_string!(i18n, common.delete).to_string() }}
                                        </button>
                                    }.into_any()
                                }}
                            })
                        } else {
                            None
                        }}
                    </div>
                })
            } else {
                None
            }}

            // ─── Reembed Migration ─────────────────────────────────
            <ReembedMigrationCard />

            </div> // scrollable content
        </div> // flex wrapper
    }
}
