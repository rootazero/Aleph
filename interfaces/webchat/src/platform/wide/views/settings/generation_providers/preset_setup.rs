//! Preset setup panel for unconfigured presets — key + endpoint quick-fill.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{GenerationProviderConfig, GenerationProvidersApi};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::preset_providers::PresetProvider;

#[component]
pub(super) fn PresetSetupPanel(
    preset: PresetProvider,
    on_added: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let api_key = RwSignal::new(String::new());
    let form_model = RwSignal::new(preset.default_model.clone());
    let base_url = RwSignal::new(preset.base_url.clone().unwrap_or_default());
    let (adding, set_adding) = signal(false);
    let (testing, set_testing) = signal(false);
    let (error_msg, set_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);

    let preset_id = preset.id.clone();
    let provider_type = preset.provider_type.clone();
    let color = preset.color.clone();
    let capabilities = preset.capabilities.clone();
    let gen_type_str = preset
        .capabilities
        .first()
        .map(crate::generation::GenerationType::as_str)
        .unwrap_or("image")
        .to_string();

    let build_config = {
        let provider_type = provider_type.clone();
        let color = color;
        let capabilities = capabilities;
        move || -> GenerationProviderConfig {
            GenerationProviderConfig {
                provider_type: provider_type.clone(),
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
                    if url.is_empty() {
                        None
                    } else {
                        Some(url)
                    }
                },
                edit_url: None,
                voices_url: None,
                models: {
                    let m = form_model.get();
                    if m.is_empty() {
                        vec![]
                    } else {
                        m.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    }
                },
                enabled: true,
                color: color.clone(),
                capabilities: capabilities.clone(),
                timeout_seconds: 120,
                verified: false,
                defaults: Default::default(),
            }
        }
    };

    let handle_test = {
        let provider_type = provider_type;
        move |_| {
            set_testing.set(true);
            set_test_result.set(None);
            set_error.set(None);
            let ptype = provider_type.clone();
            let key = api_key.get();
            let url = base_url.get();
            let mdl = form_model.get();

            spawn_local(async move {
                match GenerationProvidersApi::test_connection(
                    &state,
                    &ptype,
                    if key.is_empty() { None } else { Some(key) },
                    if url.is_empty() { None } else { Some(url) },
                    if mdl.is_empty() { None } else { Some(mdl) },
                    None, // New provider — no name yet
                )
                .await
                {
                    Ok(result) => {
                        set_testing.set(false);
                        set_test_result.set(Some((result.success, result.message)));
                    }
                    Err(e) => {
                        set_testing.set(false);
                        // Unframed on purpose — see `add_custom.rs`'s twin.
                        set_test_result.set(Some((
                            false,
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                e.to_string()
                            }),
                        )));
                    }
                }
            });
        }
    };

    let handle_add = {
        let preset_id = preset_id;
        let gen_type_str = gen_type_str;
        move |_| {
            if api_key.get().is_empty() {
                set_error.set(Some("API Key is required".to_string()));
                return;
            }
            set_adding.set(true);
            set_error.set(None);
            let config = build_config();
            let name = preset_id.clone();
            let gt = gen_type_str.clone();

            spawn_local(async move {
                match GenerationProvidersApi::create(&state, &name, config, &gt).await {
                    Ok(_) => {
                        set_adding.set(false);
                        on_added();
                    }
                    Err(e) => {
                        set_adding.set(false);
                        set_error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed: {e}")
                            }),
                        ));
                    }
                }
            });
        }
    };

    let homepage = preset.homepage.clone();
    view! {
        <div class="flex flex-col h-full">
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center gap-3">
                    <span class="text-2xl">{preset.icon.clone()}</span>
                    <div class="min-w-0">
                        <div class="flex items-center gap-2 flex-wrap">
                            <h2 class="text-lg font-semibold text-text-primary">{format!("{} {}", t_string!(i18n, settings.generation.setup_prefix), preset.name)}</h2>
                            {match homepage {
                                Some(url) if !url.is_empty() => view! {
                                    <a
                                        href=url
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        class="text-xs text-primary hover:underline shrink-0"
                                    >
                                        {"Docs ↗"}
                                    </a>
                                }.into_any(),
                                _ => view! { <span></span> }.into_any(),
                            }}
                        </div>
                        <p class="text-sm text-text-tertiary">{preset.description}</p>
                    </div>
                </div>
            </div>

            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_key_label)}</label>
                        <ProviderKeyField
                            value=api_key
                            has_api_key=Signal::derive(|| false)
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.model_label)}</label>
                        <input
                            type="text"
                            prop:value=move || form_model.get()
                            on:input=move |ev| form_model.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_endpoint_label)}</label>
                        <input
                            type="text"
                            prop:value=move || base_url.get()
                            on:input=move |ev| base_url.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
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

                // Error
                {move || error_msg.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                })}

                // Action buttons
                <div class="flex flex-row gap-3">
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
                        {move || if adding.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                    </button>
                </div>
            </div>
        </div>
    }
}
