use leptos::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;
use crate::api::{GenerationProvidersApi, GenerationProviderConfig, GenerationProviderEntry, VoiceInfo};
use crate::api::{GenerationConfig, GenerationConfigApi};
use crate::components::ui::SecretInput;
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::preset_providers::{PresetProvider, PresetProviders};
use crate::i18n::*;

/// Extract base URL from a potentially full endpoint URL.
///
/// If the URL contains a versioned API path (`/v1/`, `/v2/`, etc.),
/// strip everything from the version segment onward. Otherwise return as-is.
///
/// Examples:
/// - `https://ai.t8star.cn/v1/audio/speech` → `https://ai.t8star.cn`
/// - `https://ai.t8star.cn/v2/videos/generations` → `https://ai.t8star.cn`
/// - `https://ai.t8star.cn` → `https://ai.t8star.cn`
/// - `https://api.openai.com/v1/images/generations` → `https://api.openai.com`
fn extract_base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    // Match /v{digit}/ or /v{digit} at end — covers /v1/, /v2/, /v3/, etc.
    for (i, _) in url.match_indices("/v") {
        let rest = &url[i + 2..];
        // Check if next char is a digit followed by / or end
        if let Some(ch) = rest.chars().next() {
            if ch.is_ascii_digit() {
                // Check if followed by / or nothing more (just /vN at end)
                let after_digit = &rest[1..];
                if after_digit.is_empty() || after_digit.starts_with('/') {
                    return url[..i].to_string();
                }
            }
        }
    }
    url.to_string()
}

#[component]
pub fn GenerationProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let (providers, set_providers) = signal(Vec::<GenerationProviderEntry>::new());
    let (selected_category, set_selected_category) = signal(GenerationType::Image);
    let (selected_provider_id, set_selected_provider_id) = signal(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);
    let (is_loading, set_is_loading) = signal(true);
    let (error_message, set_error_message) = signal(Option::<String>::None);

    // Load providers on mount
    spawn_local(async move {
        match GenerationProvidersApi::list(&state).await {
            Ok(list) => {
                set_providers.set(list);
                set_is_loading.set(false);
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to load providers: {}", e)));
                set_is_loading.set(false);
            }
        }
    });

    // Reload helper
    let reload = move || {
        spawn_local(async move {
            if let Ok(list) = GenerationProvidersApi::list(&state).await {
                set_providers.set(list);
            }
        });
    };

    // Get current category presets
    let current_presets = move || PresetProviders::by_category(selected_category.get());

    // Check if a preset is configured
    let is_configured = move |preset_id: &str| {
        providers.get().iter().any(|p| p.name == preset_id)
    };

    // Get provider entry for a preset
    let get_provider_entry = move |preset_id: &str| {
        providers.get().into_iter().find(|p| p.name == preset_id)
    };

    view! {
        <div class="flex h-full">
            // Left panel - Provider list + Generation settings
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
                // Header
                <div class="px-6 py-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        {t!(i18n, settings.generation.title)}
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        {t!(i18n, settings.generation.description)}
                    </p>
                </div>

                // Category Tabs
                <div class="px-6 py-3 border-b border-border">
                    <div class="flex gap-2">
                        <CategoryTab
                            category=GenerationType::Image
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Video
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Audio
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Speech
                            selected=selected_category
                            on_select=set_selected_category
                        />
                    </div>
                </div>

                // Content
                <div class="flex-1 overflow-auto">
                    // Provider cards (loading/error/list)
                    {move || {
                        if is_loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">{t!(i18n, settings.generation.loading_providers)}</div>
                                </div>
                            }.into_any()
                        } else if let Some(error) = error_message.get() {
                            view! {
                                <div class="p-6">
                                    <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{error}</div>
                                </div>
                            }.into_any()
                        } else {
                            let presets = current_presets();
                            view! {
                                <div class="p-6 space-y-4">
                                    <div class="grid grid-cols-1 gap-2">
                                        {presets.into_iter().map(|preset| {
                                            let preset_id = preset.id.clone();
                                            let configured = is_configured(&preset_id);
                                            let entry = get_provider_entry(&preset_id);
                                            let is_selected = {
                                                let sel = selected_provider_id.get();
                                                sel.as_deref() == Some(&preset_id)
                                                    || sel.as_deref() == Some(&format!("__preset__{}", preset_id))
                                            };

                                            view! {
                                                <ProviderCard
                                                    preset=preset
                                                    is_configured=configured
                                                    entry=entry
                                                    is_selected=is_selected
                                                    on_click=move |_| {
                                                        // Configured preset → show detail; unconfigured → show setup form
                                                        if configured {
                                                            set_selected_provider_id.set(Some(preset_id.clone()));
                                                        } else {
                                                            set_selected_provider_id.set(Some(format!("__preset__{}", preset_id)));
                                                        }
                                                        set_show_add_form.set(false);
                                                    }
                                                />
                                            }
                                        }).collect_view()}
                                    </div>

                                    // Custom providers (not matching any preset in current category)
                                    {move || {
                                        let all_presets = PresetProviders::by_category(selected_category.get());
                                        let preset_ids: Vec<String> = all_presets.iter().map(|p| p.id.clone()).collect();
                                        let provider_list = providers.get();
                                        let current_cat = selected_category.get();
                                        let custom: Vec<_> = provider_list.into_iter()
                                            .filter(|p| {
                                                !preset_ids.contains(&p.name)
                                                    && p.effective_generation_type() == Some(current_cat)
                                            })
                                            .collect();
                                        if custom.is_empty() {
                                            view! { <div></div> }.into_any()
                                        } else {
                                            view! {
                                                <div class="pt-2">
                                                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                        {t!(i18n, settings.generation.custom_providers)}
                                                    </h2>
                                                    <div class="grid grid-cols-1 gap-2">
                                                        {custom.into_iter().map(|cp| {
                                                            let cp_name = cp.name.clone();
                                                            let cp_name_click = cp_name.clone();
                                                            let cp_name_check = cp_name.clone();
                                                            let cp_model = cp.config.models.first().cloned().unwrap_or_default();
                                                            let cp_color = cp.config.color.clone();
                                                            let is_default = !cp.is_default_for.is_empty();
                                                            let verified = cp.config.verified;
                                                            let first_char = cp_name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                                            view! {
                                                                <button
                                                                    on:click=move |_| {
                                                                        set_selected_provider_id.set(Some(cp_name_click.clone()));
                                                                        set_show_add_form.set(false);
                                                                    }
                                                                    class=move || {
                                                                        let base = "text-left p-3 rounded-lg border transition-all";
                                                                        let is_sel = selected_provider_id.get().as_deref() == Some(&cp_name_check);
                                                                        if is_sel {
                                                                            format!("{} bg-primary-subtle border-primary", base)
                                                                        } else {
                                                                            format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                                                        }
                                                                    }
                                                                >
                                                                    <div class="flex items-center gap-3">
                                                                        <div
                                                                            class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                                            style=format!("background-color: {}", cp_color)
                                                                        >
                                                                            {first_char}
                                                                        </div>
                                                                        <div class="min-w-0">
                                                                            <div class="flex items-center gap-2">
                                                                                <span class="font-medium text-text-primary text-sm truncate">
                                                                                    {cp_name}
                                                                                </span>
                                                                                {if is_default {
                                                                                    view! {
                                                                                        <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                                                            {t!(i18n, settings.generation.default)}
                                                                                        </span>
                                                                                    }.into_any()
                                                                                } else if verified {
                                                                                    view! {
                                                                                        <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                                                            {t!(i18n, settings.generation.active)}
                                                                                        </span>
                                                                                    }.into_any()
                                                                                } else {
                                                                                    view! { <span></span> }.into_any()
                                                                                }}
                                                                            </div>
                                                                            <div class="text-xs text-text-tertiary truncate">
                                                                                {cp_model}
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

                                    // Add Custom Provider button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                set_show_add_form.set(true);
                                                set_selected_provider_id.set(None);
                                            }
                                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                                        >
                                            {t!(i18n, settings.generation.add_custom)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

                    // Generation Settings (always visible, independent of provider loading)
                    <div class="px-6 pb-6 space-y-4">
                        <h2 class="text-lg font-semibold text-text-primary border-t border-border pt-6">
                            {t!(i18n, settings.generation.generation_settings)}
                        </h2>
                        <GenerationSettingsPanel />
                    </div>
                </div>
            </div>

            // Right panel - Provider details or Add form
            <div class="w-7/12 min-w-[320px] bg-surface">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddCustomProviderPanel
                                category=selected_category.get()
                                on_added=move || {
                                    set_show_add_form.set(false);
                                    reload();
                                }
                                on_cancel=move || set_show_add_form.set(false)
                            />
                        }.into_any()
                    } else {
                        view! {
                            <ProviderDetailPanel
                                selected_id=selected_provider_id
                                providers=providers
                                on_reload=move || reload()
                            />
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn CategoryTab(
    category: GenerationType,
    selected: ReadSignal<GenerationType>,
    on_select: WriteSignal<GenerationType>,
) -> impl IntoView {
    let is_selected = move || selected.get() == category;

    view! {
        <button
            class=move || {
                let base = "flex-1 flex flex-col items-center gap-1 px-3 py-2 rounded-lg font-medium transition-colors text-sm";
                if is_selected() {
                    format!("{} bg-info text-white", base)
                } else {
                    format!("{} bg-surface-raised text-text-secondary hover:bg-surface-sunken", base)
                }
            }
            on:click=move |_| on_select.set(category)
        >
            <span class="text-lg">{category.icon()}</span>
            <span>{category.display_name()}</span>
        </button>
    }
}

#[component]
fn ProviderCard(
    preset: PresetProvider,
    is_configured: bool,
    entry: Option<GenerationProviderEntry>,
    is_selected: bool,
    on_click: impl Fn(ev::MouseEvent) + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let is_verified = entry.as_ref().is_some_and(|e| e.config.verified);

    let is_default = move || {
        if let Some(ref e) = entry {
            !e.is_default_for.is_empty()
        } else {
            false
        }
    };

    let icon = preset.icon.clone();
    let color = preset.color.clone();
    let name = preset.name.clone();
    let model = preset.default_model.clone();
    let is_unsupported = preset.is_unsupported;

    view! {
        <button
            on:click=on_click
            class=move || {
                let base = "text-left p-3 rounded-lg border transition-all";
                if is_selected {
                    format!("{} bg-primary-subtle border-primary", base)
                } else if is_configured {
                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                } else if is_unsupported {
                    format!("{} bg-surface-sunken border-border opacity-50", base)
                } else {
                    format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                }
            }
        >
            <div class="flex items-center gap-3">
                <div
                    class="w-8 h-8 rounded-lg flex items-center justify-center text-sm shrink-0"
                    style=format!("background-color: {}", color)
                >
                    {icon}
                </div>
                <div class="min-w-0">
                    <div class="flex items-center gap-2">
                        <span class="font-medium text-text-primary text-sm truncate">
                            {name}
                        </span>
                        {move || {
                            if is_configured && is_default() {
                                view! {
                                    <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                        {t!(i18n, settings.generation.default)}
                                    </span>
                                }.into_any()
                            } else if is_configured && is_verified {
                                view! {
                                    <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                        {t!(i18n, settings.generation.active)}
                                    </span>
                                }.into_any()
                            } else if is_unsupported {
                                view! {
                                    <span class="px-1.5 py-0.5 bg-surface-sunken text-text-tertiary text-xs rounded shrink-0">
                                        {t!(i18n, settings.generation.unsupported)}
                                    </span>
                                }.into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }
                        }}
                    </div>
                    <div class="text-xs text-text-tertiary truncate">
                        {model}
                    </div>
                </div>
            </div>
        </button>
    }
}

// ============================================================================
// Provider Detail Panel
// ============================================================================

#[component]
fn ProviderDetailPanel(
    selected_id: ReadSignal<Option<String>>,
    providers: ReadSignal<Vec<GenerationProviderEntry>>,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let _state = expect_context::<DashboardState>();

    view! {
        <div class="h-full">
            {move || {
                if let Some(provider_id) = selected_id.get() {
                    // Unconfigured preset → show add form pre-filled with preset info
                    if let Some(preset_name) = provider_id.strip_prefix("__preset__") {
                        let preset = PresetProviders::all().into_iter()
                            .find(|p| p.id == preset_name);
                        if let Some(preset) = preset {
                            return view! {
                                <PresetSetupPanel
                                    preset=preset
                                    on_added=move || on_reload()
                                />
                            }.into_any();
                        }
                    }

                    // Configured provider → show editable detail
                    let provider = providers.get().into_iter()
                        .find(|p| p.name == provider_id);

                    if let Some(provider) = provider {
                        view! {
                            <ProviderDetailView
                                provider=provider
                                on_reload=on_reload
                            />
                        }.into_any()
                    } else {
                        view! {
                            <EmptyState />
                        }.into_any()
                    }
                } else {
                    view! {
                        <EmptyState />
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex flex-1 items-center justify-center h-full">
            <div class="text-center text-text-secondary">
                <p class="text-lg">{t!(i18n, settings.generation.select_provider)}</p>
            </div>
        </div>
    }
}

#[component]
fn ProviderDetailView(
    provider: GenerationProviderEntry,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let provider_name = provider.name.clone();
    let is_default_for = provider.is_default_for.clone();

    // Editable form signals
    let form_api_key = RwSignal::new(provider.config.api_key.clone().unwrap_or_default());
    let form_base_url = RwSignal::new(provider.config.base_url.clone().unwrap_or_default());
    let form_edit_url = RwSignal::new(provider.config.edit_url.clone().unwrap_or_default());
    let form_model = RwSignal::new(provider.config.models.join(","));
    let form_timeout = RwSignal::new(provider.config.timeout_seconds);
    let form_enabled = RwSignal::new(provider.config.enabled);

    // Generation type is now determined by which typed map the provider belongs to
    let effective_gen_type = provider.effective_generation_type().unwrap_or(GenerationType::Image);
    let is_speech = effective_gen_type == GenerationType::Speech;

    // Voice configuration signals (for speech providers)
    let form_voice = RwSignal::new(provider.config.defaults.voice.clone().unwrap_or_default());
    let form_speed = RwSignal::new(provider.config.defaults.speed.unwrap_or(1.0));
    let form_audio_format = RwSignal::new(provider.config.defaults.format.clone().unwrap_or_else(|| "mp3".to_string()));
    let form_stt_model = RwSignal::new(provider.config.defaults.stt_model.clone().unwrap_or_else(|| "whisper-1".to_string()));
    let voices_list: RwSignal<Vec<VoiceInfo>> = RwSignal::new(Vec::new());
    let voices_loading = RwSignal::new(false);

    // Load voices if this is a speech provider
    let provider_name_voices = provider.name.clone();
    if is_speech {
        voices_loading.set(true);
        let name = provider_name_voices.clone();
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

    let build_config = {
        let existing_defaults = existing_defaults.clone();
        move || -> GenerationProviderConfig {
            let mut defaults = existing_defaults.clone();
            // Update voice-specific defaults from form
            let voice = form_voice.get();
            defaults.voice = if voice.is_empty() { None } else { Some(voice) };
            defaults.speed = Some(form_speed.get());
            let fmt = form_audio_format.get();
            defaults.format = if fmt.is_empty() { None } else { Some(fmt) };
            let stt = form_stt_model.get();
            defaults.stt_model = if stt.is_empty() { None } else { Some(stt) };

            GenerationProviderConfig {
                provider_type: config_provider_type.clone(),
                api_key: {
                    let key = form_api_key.get();
                    if key.is_empty() { None } else { Some(key) }
                },
                secret_name: None,
                base_url: {
                    let url = extract_base_url(&form_base_url.get());
                    if url.is_empty() { None } else { Some(url) }
                },
                edit_url: {
                    let url = form_edit_url.get();
                    if url.is_empty() { None } else { Some(url) }
                },
                models: {
                    let m = form_model.get();
                    if m.is_empty() { vec![] } else { m.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() }
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
                    set_timeout(move || save_success.set(false), std::time::Duration::from_secs(2));
                    on_reload();
                }
                Err(e) => {
                    saving.set(false);
                    action_error.set(Some(format!("Save failed: {}", e)));
                }
            }
        });
    };

    // Test connection handler
    let build_config_test = build_config.clone();
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
            ).await {
                Ok(result) => {
                    testing.set(false);
                    test_result.set(Some((result.success, result.message)));
                }
                Err(e) => {
                    testing.set(false);
                    test_result.set(Some((false, e)));
                }
            }
        });
    };

    // Delete handler
    let provider_name_delete = provider_name.clone();
    let handle_delete = move |_| {
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
                    action_error.set(Some(format!("Delete failed: {}", e)));
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
                        action_error.set(Some(format!("Set default failed: {}", e)));
                    }
                }
            });
        }
    });

    let is_preset = PresetProviders::all().iter().any(|p| p.id == provider_name);

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {provider.name.clone()}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {display_provider_type.clone()}
                        </p>
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
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_key_label)}</label>
                    <SecretInput
                        value=Signal::derive(move || form_api_key.get())
                        on_change=move |v| form_api_key.set(v)
                        placeholder=t_string!(i18n, settings.generation.api_key_placeholder).to_string()
                        monospace=true
                    />
                    <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.api_key_hint)}</p>
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.model_label)}</label>
                    <input
                        type="text"
                        prop:value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder="e.g. dall-e-3, stable-diffusion-xl"
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
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"VOICE CONFIGURATION"</h3>

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
                                                <option value={vid.clone()}>
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

                        // STT Model
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.stt_model)}</label>
                            <input
                                type="text"
                                prop:value=move || form_stt_model.get()
                                on:input=move |ev| form_stt_model.set(event_target_value(&ev))
                                placeholder=t_string!(i18n, settings.generation.stt_model_placeholder).to_string()
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                            />
                            <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.generation.stt_model_hint)}</p>
                        </div>

                        // Derived Endpoints (read-only info)
                        <div class="bg-surface-sunken rounded-lg p-3 space-y-2">
                            <h4 class="text-xs font-semibold text-text-tertiary uppercase">"ENDPOINTS (auto-derived)"</h4>
                            <div class="space-y-1.5 text-xs font-mono">
                                <div class="flex gap-2">
                                    <span class="text-text-tertiary w-8 shrink-0">"TTS"</span>
                                    <span class="text-text-secondary break-all">
                                        {move || {
                                            let base = form_base_url.get();
                                            let base = extract_base_url(&base);
                                            format!("{}/v1/audio/speech", base)
                                        }}
                                    </span>
                                </div>
                                <div class="flex gap-2">
                                    <span class="text-text-tertiary w-8 shrink-0">"STT"</span>
                                    <span class="text-text-secondary break-all">
                                        {move || {
                                            let base = form_base_url.get();
                                            let base = extract_base_url(&base);
                                            format!("{}/v1/audio/transcriptions", base)
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
                let set_default = handle_set_default.clone();

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"SET AS DEFAULT"</h3>
                        <button
                            on:click=move |_| set_default(effective_gen_type)
                            disabled=move || setting_default.get() || is_default
                            class=move || {
                                let base = "w-full px-4 py-2.5 rounded-lg transition-colors font-medium text-sm";
                                if is_default {
                                    format!("{} bg-primary-subtle text-primary cursor-not-allowed", base)
                                } else {
                                    format!("{} bg-surface-sunken text-text-secondary hover:bg-surface-raised disabled:opacity-50", base)
                                }
                            }
                        >
                            {effective_gen_type.display_name()}
                            {if is_default { format!(" {}", t_string!(i18n, settings.generation.current_suffix)) } else { String::new() }}
                        </button>
                    </div>
                }
            }

            // Delete button
            {if !is_preset {
                view! {
                    <button
                        on:click=handle_delete
                        disabled=move || deleting.get()
                        class="w-full px-4 py-2.5 bg-danger-subtle text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors font-medium"
                    >
                        {move || if deleting.get() { t_string!(i18n, settings.generation.deleting).to_string() } else { t_string!(i18n, settings.generation.delete_provider).to_string() }}
                    </button>
                }.into_any()
            } else {
                view! { <span></span> }.into_any()
            }}

            </div> // scrollable content
        </div> // flex wrapper
    }
}

// ============================================================================
// Preset Setup Panel (for unconfigured presets)
// ============================================================================

#[component]
fn PresetSetupPanel(
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
    let gen_type_str = preset.capabilities.first().map(|g| g.as_str()).unwrap_or("image").to_string();

    let build_config = {
        let provider_type = provider_type.clone();
        let color = color.clone();
        let capabilities = capabilities.clone();
        move || -> GenerationProviderConfig {
            GenerationProviderConfig {
                provider_type: provider_type.clone(),
                api_key: {
                    let key = api_key.get();
                    if key.is_empty() { None } else { Some(key) }
                },
                secret_name: None,
                base_url: {
                    let url = base_url.get();
                    if url.is_empty() { None } else { Some(url) }
                },
                edit_url: None,
                models: {
                    let m = form_model.get();
                    if m.is_empty() { vec![] } else { m.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() }
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
        let provider_type = provider_type.clone();
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
                ).await {
                    Ok(result) => {
                        set_testing.set(false);
                        set_test_result.set(Some((result.success, result.message)));
                    }
                    Err(e) => {
                        set_testing.set(false);
                        set_test_result.set(Some((false, e)));
                    }
                }
            });
        }
    };

    let handle_add = {
        let preset_id = preset_id.clone();
        let gen_type_str = gen_type_str.clone();
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
                        set_error.set(Some(format!("Failed: {}", e)));
                    }
                }
            });
        }
    };

    view! {
        <div class="flex flex-col h-full">
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center gap-3">
                    <span class="text-2xl">{preset.icon.clone()}</span>
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">{format!("{} {}", t_string!(i18n, settings.generation.setup_prefix), preset.name)}</h2>
                        <p class="text-sm text-text-tertiary">{preset.description.clone()}</p>
                    </div>
                </div>
            </div>

            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.generation.api_key_label)}</label>
                        <SecretInput
                            value=Signal::derive(move || api_key.get())
                            on_change=move |v| api_key.set(v)
                            placeholder=t_string!(i18n, settings.generation.api_key_setup_placeholder).to_string()
                            monospace=true
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

// ============================================================================
// Add Custom Provider Panel
// ============================================================================

#[component]
fn AddCustomProviderPanel(
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
                if key.is_empty() { None } else { Some(key) }
            },
            secret_name: None,
            base_url: {
                let url = base_url.get();
                let url = extract_base_url(&url);
                if url.is_empty() { None } else { Some(url) }
            },
            edit_url: {
                let url = edit_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            models: if form_model.get().is_empty() { vec![] } else { form_model.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() },
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
            match GenerationProvidersApi::test_connection(&state, &ptype, key, url, mdl, None).await {
                Ok(result) => {
                    set_testing.set(false);
                    set_test_result.set(Some((result.success, result.message)));
                }
                Err(e) => {
                    set_testing.set(false);
                    set_test_result.set(Some((false, e)));
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
                    set_add_error.set(Some(format!("Failed to add: {}", e)));
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
                    <SecretInput
                        value=Signal::derive(move || api_key.get())
                        on_change=move |v| api_key.set(v)
                        placeholder=t_string!(i18n, settings.providers.api_key_placeholder).to_string()
                        monospace=true
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

// ============================================================================
// Generation Settings Panel (merged from Generation view)
// ============================================================================

#[component]
fn GenerationSettingsPanel() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(GenerationConfig {
        default_image_provider: None,
        default_video_provider: None,
        default_audio_provider: None,
        default_speech_provider: None,
        output_dir: String::new(),
        auto_paste_threshold_mb: 5,
        background_task_threshold_seconds: 30,
        smart_routing_enabled: true,
    });
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    spawn_local(async move {
        match GenerationConfigApi::get(&state).await {
            Ok(cfg) => {
                config.set(cfg);
                loading.set(false);
            }
            Err(_) => {
                loading.set(false);
            }
        }
    });

    let output_dir = RwSignal::new(String::new());
    let auto_paste = RwSignal::new(5u32);
    let bg_threshold = RwSignal::new(30u32);
    let smart_routing = RwSignal::new(true);

    // Sync local signals when config loads
    Effect::new(move || {
        if !loading.get() {
            let cfg = config.get();
            output_dir.set(cfg.output_dir);
            auto_paste.set(cfg.auto_paste_threshold_mb);
            bg_threshold.set(cfg.background_task_threshold_seconds);
            smart_routing.set(cfg.smart_routing_enabled);
        }
    });

    let save = move |_| {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.output_dir = output_dir.get();
        cfg.auto_paste_threshold_mb = auto_paste.get();
        cfg.background_task_threshold_seconds = bg_threshold.get();
        cfg.smart_routing_enabled = smart_routing.get();

        spawn_local(async move {
            match GenerationConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(move || save_success.set(false), std::time::Duration::from_secs(2));
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    };

    view! {
        {move || {
            if loading.get() {
                view! {
                    <div class="text-text-tertiary text-sm">{t!(i18n, settings.generation.loading_settings)}</div>
                }.into_any()
            } else {
                view! {
                    <div class="space-y-4">
                        // Thresholds
                        <div class="bg-surface-raised rounded-lg border border-border p-4 space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    {t!(i18n, settings.generation.auto_paste_label)} ": " {move || auto_paste.get()} " " {t!(i18n, settings.generation.auto_paste_unit)}
                                </label>
                                <input
                                    type="range" min="1" max="100" step="1"
                                    value=move || auto_paste.get()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<u32>() { auto_paste.set(v); }
                                    }
                                    class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                />
                                <p class="mt-1 text-xs text-text-tertiary">
                                    {t!(i18n, settings.generation.auto_paste_hint)}
                                </p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    {t!(i18n, settings.generation.bg_threshold_label)} ": " {move || bg_threshold.get()} " " {t!(i18n, settings.generation.bg_threshold_unit)}
                                </label>
                                <input
                                    type="range" min="1" max="300" step="5"
                                    value=move || bg_threshold.get()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<u32>() { bg_threshold.set(v); }
                                    }
                                    class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                />
                                <p class="mt-1 text-xs text-text-tertiary">
                                    {t!(i18n, settings.generation.bg_threshold_hint)}
                                </p>
                            </div>
                        </div>

                        // Smart Routing
                        <div class="bg-surface-raised rounded-lg border border-border p-4">
                            <label class="flex items-center gap-3 cursor-pointer">
                                <input
                                    type="checkbox"
                                    checked=move || smart_routing.get()
                                    on:change=move |ev| smart_routing.set(event_target_checked(&ev))
                                    class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                                />
                                <div>
                                    <div class="text-sm font-medium text-text-primary">{t!(i18n, settings.generation.smart_routing)}</div>
                                    <div class="text-xs text-text-tertiary">
                                        {t!(i18n, settings.generation.smart_routing_hint)}
                                    </div>
                                </div>
                            </label>
                        </div>

                        // Save feedback
                        {move || save_error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{e}</div>
                        })}
                        {move || save_success.get().then(|| view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">{t!(i18n, common.saved)}</div>
                        })}

                        // Save button
                        <button
                            on:click=save
                            disabled=move || saving.get()
                            class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                        >
                            {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                        </button>
                    </div>
                }.into_any()
            }
        }}
    }
}
