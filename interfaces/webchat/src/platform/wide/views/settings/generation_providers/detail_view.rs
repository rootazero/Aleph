//! Provider detail panel — shows config for a single generation provider.

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;

use crate::api::{
    GenerationProviderConfig, GenerationProviderEntry, GenerationProvidersApi, VoiceInfo,
};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::i18n::{t, t_string, use_i18n};
use crate::preset_providers::PresetCatalog;

use super::extract_base_url;

#[component]
pub(super) fn ProviderDetailView(
    provider: GenerationProviderEntry,
    catalog: ReadSignal<PresetCatalog>,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let provider_name = provider.name.clone();
    let is_default_for = provider.is_default_for.clone();

    // Editable form signals
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(provider.config.base_url.clone().unwrap_or_default());
    let form_edit_url = RwSignal::new(provider.config.edit_url.clone().unwrap_or_default());
    let form_model = RwSignal::new(provider.config.models.join(","));
    let form_timeout = RwSignal::new(provider.config.timeout_seconds);
    let form_enabled = RwSignal::new(provider.config.enabled);

    // Generation type is now determined by which typed map the provider belongs to
    let effective_gen_type = provider
        .effective_generation_type()
        .unwrap_or(GenerationType::Image);
    let is_speech = effective_gen_type == GenerationType::Speech;

    // Voice configuration signals (for speech providers)
    let form_voice = RwSignal::new(provider.config.defaults.voice.clone().unwrap_or_default());
    let form_speed = RwSignal::new(provider.config.defaults.speed.unwrap_or(1.0));
    let form_audio_format = RwSignal::new(
        provider
            .config
            .defaults
            .format
            .clone()
            .unwrap_or_else(|| "mp3".to_string()),
    );
    let form_voices_url = RwSignal::new(provider.config.voices_url.clone().unwrap_or_default());
    let voices_list: RwSignal<Vec<VoiceInfo>> = RwSignal::new(Vec::new());
    let voices_loading = RwSignal::new(false);

    // Load voices if this is a speech provider
    let provider_name_voices = provider.name.clone();
    if is_speech {
        voices_loading.set(true);
        let name = provider_name_voices;
        spawn_local(async move {
            match GenerationProvidersApi::fetch_voices(&state, &name).await {
                Ok(list) => {
                    voices_list.set(list);
                    voices_loading.set(false);
                }
                Err(_) => {
                    voices_loading.set(false);
                }
            }
        });
    }

    // Preserve existing defaults for non-voice fields
    let existing_defaults = provider.config.defaults.clone();

    // Action state
    let saving = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let testing = RwSignal::new(false);
    let setting_default = RwSignal::new(false);
    let action_error = RwSignal::new(Option::<String>::None);
    let test_result = RwSignal::new(Option::<(bool, String)>::None);
    let save_success = RwSignal::new(false);

    // Pre-clone values captured by build_config closure
    let config_provider_type = provider.config.provider_type.clone();
    let display_provider_type = provider.config.provider_type.clone();
    let config_color = provider.config.color.clone();
    let config_verified = provider.config.verified;
    let provider_has_api_key = provider.has_api_key;

    let build_config = {
        let existing_defaults = existing_defaults;
        move || -> GenerationProviderConfig {
            let mut defaults = existing_defaults.clone();
            // Update voice-specific defaults from form
            let voice = form_voice.get();
            defaults.voice = if voice.is_empty() { None } else { Some(voice) };
            defaults.speed = Some(form_speed.get());
            let fmt = form_audio_format.get();
            defaults.format = if fmt.is_empty() { None } else { Some(fmt) };
            GenerationProviderConfig {
                provider_type: config_provider_type.clone(),
                api_key: {
                    let key = form_api_key.get();
                    if key.is_empty() {
                        None
                    } else {
                        Some(key)
                    }
                },
                secret_name: None,
                base_url: {
                    let url = extract_base_url(&form_base_url.get());
                    if url.is_empty() {
                        None
                    } else {
                        Some(url)
                    }
                },
                edit_url: {
                    let url = form_edit_url.get();
                    if url.is_empty() {
                        None
                    } else {
                        Some(url)
                    }
                },
                voices_url: {
                    let url = form_voices_url.get();
                    if url.is_empty() {
                        None
                    } else {
                        Some(url)
                    }
                },
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
                enabled: form_enabled.get(),
                color: config_color.clone(),
                capabilities: vec![effective_gen_type],
                timeout_seconds: form_timeout.get(),
                verified: config_verified,
                defaults,
            }
        }
    };

    let build_config = Rc::new(build_config);

    // Save handler
    let provider_name_save = provider_name.clone();
    let build_config_save = build_config.clone();
    let on_save = move |_| {
        saving.set(true);
        action_error.set(None);
        save_success.set(false);
        let config = build_config_save();
        let name = provider_name_save.clone();

        spawn_local(async move {
            match GenerationProvidersApi::update(&state, &name, config).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                    on_reload();
                }
                Err(e) => {
                    saving.set(false);
                    action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Save failed: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Test connection handler
    let build_config_test = build_config;
    let provider_name_test = provider_name.clone();
    let handle_test = move |_| {
        testing.set(true);
        test_result.set(None);
        action_error.set(None);
        let config = build_config_test();
        let name = provider_name_test.clone();

        spawn_local(async move {
            match GenerationProvidersApi::test_connection(
                &state,
                &config.provider_type,
                config.api_key,
                config.base_url,
                config.models.first().cloned(),
                Some(&name),
            )
            .await
            {
                Ok(result) => {
                    testing.set(false);
                    test_result.set(Some((result.success, result.message)));
                    if result.success {
                        on_reload();
                    }
                }
                Err(e) => {
                    testing.set(false);
                    // Unframed on purpose — see `add_custom.rs`'s twin.
                    test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    )));
                }
            }
        });
    };

    // Delete handler
    let provider_name_delete = provider_name.clone();
    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
        let name = provider_name_delete.clone();
        deleting.set(true);
        action_error.set(None);

        spawn_local(async move {
            match GenerationProvidersApi::delete(&state, &name).await {
                Ok(_) => {
                    deleting.set(false);
                    on_reload();
                }
                Err(e) => {
                    deleting.set(false);
                    action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Delete failed: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Set default handler
    let provider_name_default = provider_name.clone();
    let handle_set_default = Rc::new({
        let name = provider_name_default;
        move |gen_type: GenerationType| {
            let name = name.clone();
            setting_default.set(true);
            action_error.set(None);

            spawn_local(async move {
                match GenerationProvidersApi::set_default(&state, &name, gen_type).await {
                    Ok(_) => {
                        setting_default.set(false);
                        on_reload();
                    }
                    Err(e) => {
                        setting_default.set(false);
                        action_error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Set default failed: {e}")
                            }),
                        ));
                    }
                }
            });
        }
    });

    let is_preset = catalog.get().is_preset(&provider_name);

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {provider.name}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {display_provider_type}
                        </p>
                        <ProviderBadges state=BadgeState {
                            is_default: !is_default_for.is_empty(),
                            verified: config_verified,
                        } />
                    </div>
                    <label class="flex items-center gap-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || form_enabled.get()
                            on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                            class="w-4 h-4 rounded"
                        />
                        <span class="text-sm text-text-secondary">{t!(i18n, settings.generation.enabled_label)}</span>
                    </label>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration form card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.generation.configuration_header)}</h3>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_key_label)}</label>
                    <ProviderKeyField
                        value=form_api_key
                        has_api_key=Signal::derive(move || provider_has_api_key)
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.model_label)}</label>
                    <input
                        type="text"
                        prop:value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder=t_string!(i18n, settings.generation.model_placeholder)
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // API Endpoint URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_endpoint_label)}</label>
                    <input
                        type="text"
                        prop:value=move || form_base_url.get()
                        on:input=move |ev| form_base_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/generations"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Edit Endpoint URL (optional)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.edit_endpoint_label)}</label>
                    <input
                        type="text"
                        prop:value=move || form_edit_url.get()
                        on:input=move |ev| form_edit_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/edits"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.edit_endpoint_hint)}</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.generation.timeout_label)} ": " {move || form_timeout.get()} "s"
                    </label>
                    <input
                        type="range" min="10" max="600" step="10"
                        prop:value=move || form_timeout.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() { form_timeout.set(v); }
                        }
                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                </div>
            </div>

            // Voice Configuration card (only shown for speech providers)
            {move || {
                if !is_speech {
                    return view! { <div></div> }.into_any();
                }

                let current_voices = voices_list.get();
                let is_loading = voices_loading.get();

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.generation.voice_config_header)}</h3>

                        // Default Voice dropdown
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.default_voice)}</label>
                            {if is_loading {
                                view! {
                                    <div class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-tertiary">
                                        {t!(i18n, settings.generation.loading_voices)}
                                    </div>
                                }.into_any()
                            } else if current_voices.is_empty() {
                                view! {
                                    <input
                                        type="text"
                                        prop:value=move || form_voice.get()
                                        on:input=move |ev| form_voice.set(event_target_value(&ev))
                                        placeholder=t_string!(i18n, settings.generation.voice_id_placeholder).to_string()
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <select
                                        prop:value=move || form_voice.get()
                                        on:change=move |ev| form_voice.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                    >
                                        <option value="">{t!(i18n, settings.generation.select_voice)}</option>
                                        {current_voices.iter().map(|v| {
                                            let vid = v.id.clone();
                                            let label = if v.gender.is_empty() {
                                                v.name.clone()
                                            } else {
                                                format!("{} ({})", v.name, v.gender)
                                            };
                                            view! {
                                                <option value={vid}>
                                                    {label}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </select>
                                }.into_any()
                            }}
                        </div>

                        // Default Speed slider
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                {t!(i18n, settings.generation.speed_label)} ": " {move || format!("{:.2}x", form_speed.get())}
                            </label>
                            <input
                                type="range"
                                min="0.25" max="4.0" step="0.25"
                                prop:value=move || form_speed.get().to_string()
                                on:input=move |ev| {
                                    if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                        form_speed.set(v);
                                    }
                                }
                                class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                            />
                            <div class="flex justify-between text-xs text-text-tertiary mt-1">
                                <span>"0.25x"</span>
                                <span>"1.0x"</span>
                                <span>"4.0x"</span>
                            </div>
                        </div>

                        // Default Format dropdown
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.output_format)}</label>
                            <select
                                prop:value=move || form_audio_format.get()
                                on:change=move |ev| form_audio_format.set(event_target_value(&ev))
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                            >
                                <option value="mp3">"MP3"</option>
                                <option value="opus">"Opus"</option>
                                <option value="aac">"AAC"</option>
                                <option value="flac">"FLAC"</option>
                            </select>
                        </div>

                        // Voices URL
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.voices_url)}</label>
                            <input
                                type="text"
                                prop:value=move || form_voices_url.get()
                                on:input=move |ev| form_voices_url.set(event_target_value(&ev))
                                placeholder="https://example.com/v1/audio/voices"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                            />
                            <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.voices_url_hint)}</p>
                        </div>

                        // Derived Endpoints (read-only info)
                        <div class="bg-surface-sunken rounded-lg p-3 space-y-2">
                            <h4 class="text-xs font-semibold text-text-tertiary uppercase">{t!(i18n, settings.generation.endpoints_header)}</h4>
                            <div class="space-y-1.5 text-xs font-mono">
                                <div class="flex gap-2">
                                    <span class="text-text-tertiary w-8 shrink-0">"TTS"</span>
                                    <span class="text-text-secondary break-all">
                                        {move || {
                                            let base = form_base_url.get();
                                            let base = extract_base_url(&base);
                                            format!("{base}/v1/audio/speech")
                                        }}
                                    </span>
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}

            // Test result feedback
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

            // Save success feedback
            {move || save_success.get().then(|| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{t!(i18n, settings.generation.saved_successfully)}</div>
            })}

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Action buttons: Test + Save
            <div class="flex flex-row gap-3">
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if testing.get() { t_string!(i18n, settings.generation.testing).to_string() } else { t_string!(i18n, settings.generation.test_connection).to_string() }}
                </button>
                <button
                    on:click=on_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>

            // Set as default button
            {
                let is_default = is_default_for.contains(&effective_gen_type);
                let set_default = handle_set_default;

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.generation.set_as_default_header)}</h3>
                        <button
                            on:click=move |_| set_default(effective_gen_type)
                            disabled=move || setting_default.get() || is_default || !config_verified
                            class=move || {
                                let base = "w-full px-4 py-2.5 rounded-lg transition-colors font-medium text-sm";
                                if is_default {
                                    format!("{base} bg-primary-subtle text-primary cursor-not-allowed")
                                } else {
                                    format!("{base} bg-surface-sunken text-text-secondary hover:bg-surface-raised disabled:opacity-50")
                                }
                            }
                        >
                            {effective_gen_type.display_name()}
                            {if is_default { format!(" {}", t_string!(i18n, settings.generation.current_suffix)) } else { String::new() }}
                        </button>
                        {(!config_verified).then(|| view! {
                            <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.verify_before_default)}</p>
                        })}
                    </div>
                }
            }

            // Delete button (two-step inline confirm)
            {if !is_preset {
                view! {
                    {move || if confirming.get() {
                        view! {
                            <ConfirmButton confirming=confirming on_confirm=on_confirm_delete.clone() width_class="w-full" />
                        }.into_any()
                    } else {
                        view! {
                            <button
                                on:click=move |_| confirming.set(true)
                                disabled=move || deleting.get()
                                class="w-full px-4 py-2.5 bg-danger-subtle text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors font-medium"
                            >
                                {move || if deleting.get() { t_string!(i18n, settings.generation.deleting).to_string() } else { t_string!(i18n, settings.generation.delete_provider).to_string() }}
                            </button>
                        }.into_any()
                    }}
                }.into_any()
            } else {
                view! { <span></span> }.into_any()
            }}

            </div> // scrollable content
        </div> // flex wrapper
    }
}
