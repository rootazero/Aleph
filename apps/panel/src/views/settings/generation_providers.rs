use leptos::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;
use crate::api::{GenerationProvidersApi, GenerationProviderConfig, GenerationProviderEntry};
use crate::api::{GenerationConfig, GenerationConfigApi};
use crate::components::ui::SecretInput;
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::preset_providers::{PresetProvider, PresetProviders};


#[component]
pub fn GenerationProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

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
                        "Generation Providers"
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        "Configure media generation providers and settings"
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
                    </div>
                </div>

                // Content
                <div class="flex-1 overflow-auto">
                    // Provider cards (loading/error/list)
                    {move || {
                        if is_loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">"Loading providers..."</div>
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
                                    <div class="grid grid-cols-1 gap-3">
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
                                                    && p.config.capabilities.contains(&current_cat)
                                            })
                                            .collect();
                                        if custom.is_empty() {
                                            view! { <div></div> }.into_any()
                                        } else {
                                            view! {
                                                <div class="pt-2">
                                                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                        "Custom Providers"
                                                    </h2>
                                                    <div class="grid grid-cols-1 gap-3">
                                                        {custom.into_iter().map(|cp| {
                                                            let cp_name = cp.name.clone();
                                                            let cp_name_click = cp_name.clone();
                                                            let cp_name_check = cp_name.clone();
                                                            let cp_model = cp.config.model.clone().unwrap_or_default();
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
                                                                                            "Default"
                                                                                        </span>
                                                                                    }.into_any()
                                                                                } else if verified {
                                                                                    view! {
                                                                                        <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                                                            "Active"
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
                                            "+ Add Custom Provider"
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

                    // Generation Settings (always visible, independent of provider loading)
                    <div class="px-6 pb-6 space-y-4">
                        <h2 class="text-lg font-semibold text-text-primary border-t border-border pt-6">
                            "Generation Settings"
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
                let base = "px-4 py-2 rounded-lg font-medium transition-colors";
                if is_selected() {
                    format!("{} bg-info text-white", base)
                } else {
                    format!("{} bg-surface-raised text-text-secondary hover:bg-surface-sunken", base)
                }
            }
            on:click=move |_| on_select.set(category)
        >
            <span class="mr-2">{category.icon()}</span>
            {category.display_name()}
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
    let is_verified = entry.as_ref().map_or(false, |e| e.config.verified);

    let is_default = move || {
        if let Some(ref e) = entry {
            !e.is_default_for.is_empty()
        } else {
            false
        }
    };

    view! {
        <div
            class=move || {
                let base = "border rounded-lg p-4 hover:border-primary cursor-pointer transition-colors";
                let selected = if is_selected { " ring-2 ring-primary/30 border-primary bg-primary-subtle" } else { " border-border" };
                let opacity = if preset.is_unsupported { " opacity-50" } else { "" };
                format!("{}{}{}", base, selected, opacity)
            }
            on:click=on_click
        >
            <div class="flex items-start justify-between mb-3">
                <div class="flex items-center gap-2">
                    <span class="text-2xl">{preset.icon.clone()}</span>
                    <div>
                        <h3 class="font-semibold text-text-primary">
                            {preset.name.clone()}
                        </h3>
                        {preset.is_unsupported.then(|| view! {
                            <span class="text-xs text-text-tertiary">"(Unsupported)"</span>
                        })}
                    </div>
                </div>
                {move || {
                    if is_configured {
                        if is_default() {
                            view! {
                                <div class="flex gap-1">
                                    <span class="px-2 py-1 text-xs font-medium bg-primary-subtle text-primary rounded">
                                        "Default"
                                    </span>
                                    {if is_verified {
                                        view! {
                                            <span class="px-2 py-1 text-xs font-medium bg-success-subtle text-success rounded">
                                                "Active"
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }}
                                </div>
                            }.into_any()
                        } else if is_verified {
                            view! {
                                <span class="px-2 py-1 text-xs font-medium bg-success-subtle text-success rounded">
                                    "Active"
                                </span>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }
                    } else {
                        view! { <span></span> }.into_any()
                    }
                }}
            </div>

            <p class="text-sm text-text-secondary mb-3">
                {preset.description.clone()}
            </p>

            <div class="flex items-center gap-2 text-xs text-text-tertiary">
                <span class="font-mono">{preset.default_model.clone()}</span>
            </div>
        </div>
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
    view! {
        <div class="flex flex-1 items-center justify-center h-full">
            <div class="text-center text-text-secondary">
                <p class="text-lg">"Select a provider to view details"</p>
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

    let provider_name = provider.name.clone();
    let is_default_for = provider.is_default_for.clone();

    // Editable form signals
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(provider.config.base_url.clone().unwrap_or_default());
    let form_edit_url = RwSignal::new(provider.config.edit_url.clone().unwrap_or_default());
    let form_model = RwSignal::new(provider.config.model.clone().unwrap_or_default());
    let form_timeout = RwSignal::new(provider.config.timeout_seconds);
    let form_enabled = RwSignal::new(provider.config.enabled);

    // Capability checkboxes
    let cap_image = RwSignal::new(provider.config.capabilities.contains(&GenerationType::Image));
    let cap_video = RwSignal::new(provider.config.capabilities.contains(&GenerationType::Video));
    let cap_audio = RwSignal::new(provider.config.capabilities.contains(&GenerationType::Audio));
    let cap_speech = RwSignal::new(provider.config.capabilities.contains(&GenerationType::Speech));

    // Action state
    let saving = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let testing = RwSignal::new(false);
    let setting_default = RwSignal::new(false);
    let action_error = RwSignal::new(Option::<String>::None);
    let test_result = RwSignal::new(Option::<(bool, String)>::None);
    let save_success = RwSignal::new(false);

    let build_capabilities = move || {
        let mut caps = Vec::new();
        if cap_image.get() { caps.push(GenerationType::Image); }
        if cap_video.get() { caps.push(GenerationType::Video); }
        if cap_audio.get() { caps.push(GenerationType::Audio); }
        if cap_speech.get() { caps.push(GenerationType::Speech); }
        caps
    };

    // Pre-clone values captured by build_config closure
    let config_provider_type = provider.config.provider_type.clone();
    let display_provider_type = provider.config.provider_type.clone();
    let config_color = provider.config.color.clone();
    let config_verified = provider.config.verified;

    let build_config = move || -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: config_provider_type.clone(),
            api_key: {
                let key = form_api_key.get();
                if key.is_empty() { None } else { Some(key) }
            },
            secret_name: None,
            base_url: {
                let url = form_base_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            edit_url: {
                let url = form_edit_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            model: {
                let m = form_model.get();
                if m.is_empty() { None } else { Some(m) }
            },
            enabled: form_enabled.get(),
            color: config_color.clone(),
            capabilities: build_capabilities(),
            timeout_seconds: form_timeout.get(),
            verified: config_verified,
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
    let handle_test = move |_| {
        testing.set(true);
        test_result.set(None);
        action_error.set(None);
        let config = build_config_test();

        spawn_local(async move {
            match GenerationProvidersApi::test_connection(
                &state,
                &config.provider_type,
                config.api_key,
                config.base_url,
                config.model,
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

    let capabilities = provider.config.capabilities.clone();
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
                        <span class="text-sm text-text-secondary">"Enabled"</span>
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
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Key"</label>
                    <SecretInput
                        value=Signal::derive(move || form_api_key.get())
                        on_change=move |v| form_api_key.set(v)
                        placeholder="Enter new key to update (leave empty to keep current)"
                        monospace=true
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"Leave empty to keep existing key"</p>
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Model"</label>
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
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Endpoint URL"</label>
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
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Edit Endpoint URL (optional)"</label>
                    <input
                        type="text"
                        prop:value=move || form_edit_url.get()
                        on:input=move |ev| form_edit_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/edits"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"For image editing. Leave empty to auto-derive."</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Timeout: " {move || form_timeout.get()} "s"
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

            // Capabilities card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CAPABILITIES"</h3>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_image.get()
                        on:change=move |ev| cap_image.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"Image Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_video.get()
                        on:change=move |ev| cap_video.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"Video Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_audio.get()
                        on:change=move |ev| cap_audio.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"Audio Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_speech.get()
                        on:change=move |ev| cap_speech.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"Speech Synthesis"</span>
                </label>
            </div>

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
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">"Saved successfully"</div>
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
                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                </button>
                <button
                    on:click=on_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>

            // Set as default buttons
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"SET AS DEFAULT FOR"</h3>
                {capabilities.iter().map(|cap| {
                    let gen_type = *cap;
                    let is_default = is_default_for.contains(&gen_type);
                    let set_default = handle_set_default.clone();

                    view! {
                        <button
                            on:click=move |_| set_default(gen_type)
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
                            {gen_type.display_name()}
                            {if is_default { " (Current)" } else { "" }}
                        </button>
                    }
                }).collect_view()}
            </div>

            // Delete button
            {if !is_preset {
                view! {
                    <button
                        on:click=handle_delete
                        disabled=move || deleting.get()
                        class="w-full px-4 py-2.5 bg-danger-subtle text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors font-medium"
                    >
                        {move || if deleting.get() { "Deleting..." } else { "Delete Provider" }}
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
                model: {
                    let m = form_model.get();
                    if m.is_empty() { None } else { Some(m) }
                },
                enabled: true,
                color: color.clone(),
                capabilities: capabilities.clone(),
                timeout_seconds: 120,
                verified: false,
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
        move |_| {
            if api_key.get().is_empty() {
                set_error.set(Some("API Key is required".to_string()));
                return;
            }
            set_adding.set(true);
            set_error.set(None);
            let config = build_config();
            let name = preset_id.clone();

            spawn_local(async move {
                match GenerationProvidersApi::create(&state, &name, config).await {
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
                        <h2 class="text-lg font-semibold text-text-primary">{format!("Setup {}", preset.name)}</h2>
                        <p class="text-sm text-text-tertiary">{preset.description.clone()}</p>
                    </div>
                </div>
            </div>

            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"API Key"</label>
                        <SecretInput
                            value=Signal::derive(move || api_key.get())
                            on_change=move |v| api_key.set(v)
                            placeholder="Enter your API key"
                            monospace=true
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Model"</label>
                        <input
                            type="text"
                            prop:value=move || form_model.get()
                            on:input=move |ev| form_model.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"API Endpoint URL"</label>
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
                        {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                    </button>
                    <button
                        on:click=handle_add
                        disabled=move || adding.get()
                        class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                    >
                        {move || if adding.get() { "Saving..." } else { "Save" }}
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
    on_added: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form state
    let name = RwSignal::new(String::new());
    let provider_type = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let base_url = RwSignal::new(String::new());
    let edit_url = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let timeout = RwSignal::new(60u64);

    // Capability checkboxes
    let cap_image = RwSignal::new(true);
    let cap_video = RwSignal::new(false);
    let cap_audio = RwSignal::new(false);
    let cap_speech = RwSignal::new(false);

    let (adding, set_adding) = signal(false);
    let (testing, set_testing) = signal(false);
    let (add_error, set_add_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);

    let build_capabilities = move || {
        let mut caps = Vec::new();
        if cap_image.get() { caps.push(GenerationType::Image); }
        if cap_video.get() { caps.push(GenerationType::Video); }
        if cap_audio.get() { caps.push(GenerationType::Audio); }
        if cap_speech.get() { caps.push(GenerationType::Speech); }
        caps
    };

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
                if url.is_empty() { None } else { Some(url) }
            },
            edit_url: {
                let url = edit_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            model: if form_model.get().is_empty() { None } else { Some(form_model.get()) },
            enabled: true,
            color: "#808080".to_string(),
            capabilities: build_capabilities(),
            timeout_seconds: timeout.get(),
            verified: false,
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
        let mdl = config.model.clone();

        spawn_local(async move {
            match GenerationProvidersApi::test_connection(&state, &ptype, key, url, mdl).await {
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

        spawn_local(async move {
            match GenerationProvidersApi::create(&state, &n, config).await {
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
                    <h2 class="text-xl font-semibold text-text-primary">"Add Custom Provider"</h2>
                    <button
                        on:click=move |_| on_cancel()
                        class="text-text-tertiary hover:text-text-primary transition-colors"
                    >
                        "Cancel"
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
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Provider Name"</label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder="e.g., my-dalle"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"Unique identifier (lowercase, no spaces)"</p>
                </div>

                // Provider Type
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Provider Type"</label>
                    <input
                        type="text"
                        value=move || provider_type.get()
                        on:input=move |ev| provider_type.set(event_target_value(&ev))
                        placeholder="e.g., openai, replicate, stability"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Key"</label>
                    <SecretInput
                        value=Signal::derive(move || api_key.get())
                        on_change=move |v| api_key.set(v)
                        placeholder="sk-..."
                        monospace=true
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Model"</label>
                    <input
                        type="text"
                        value=move || form_model.get()
                        on:input=move |ev| form_model.set(event_target_value(&ev))
                        placeholder="e.g. dall-e-3, stable-diffusion-xl"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"Enter multiple models, separated by commas"</p>
                </div>

                // API Endpoint URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Endpoint URL"</label>
                    <input
                        type="text"
                        value=move || base_url.get()
                        on:input=move |ev| base_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/generations"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Edit Endpoint URL (optional)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Edit Endpoint URL (optional)"</label>
                    <input
                        type="text"
                        value=move || edit_url.get()
                        on:input=move |ev| edit_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1/images/edits"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"For image editing. Leave empty to auto-derive."</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Timeout: " {move || timeout.get()} "s"
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

            // Capabilities
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CAPABILITIES"</h3>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_image.get()
                        on:change=move |ev| cap_image.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"🖼️ Image Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_video.get()
                        on:change=move |ev| cap_video.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"🎬 Video Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_audio.get()
                        on:change=move |ev| cap_audio.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"🎵 Audio Generation"</span>
                </label>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox"
                        checked=move || cap_speech.get()
                        on:change=move |ev| cap_speech.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <span class="text-sm text-text-primary">"🗣️ Speech Synthesis"</span>
                </label>
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
                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                </button>

                <button
                    on:click=handle_add
                    disabled=move || adding.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if adding.get() { "Adding..." } else { "Add Provider" }}
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
                    <div class="text-text-tertiary text-sm">"Loading settings..."</div>
                }.into_any()
            } else {
                view! {
                    <div class="space-y-4">
                        // Output Directory
                        <div class="bg-surface-raised rounded-lg border border-border p-4">
                            <label class="block text-sm font-medium text-text-secondary mb-2">
                                "Output Directory"
                            </label>
                            <input
                                type="text"
                                value=move || output_dir.get()
                                on:input=move |ev| output_dir.set(event_target_value(&ev))
                                placeholder="~/.aleph/generation"
                                class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                            />
                            <p class="mt-1 text-xs text-text-tertiary">
                                "Where generated files (images, videos, audio) will be saved"
                            </p>
                        </div>

                        // Thresholds
                        <div class="bg-surface-raised rounded-lg border border-border p-4 space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    "Auto-paste threshold: " {move || auto_paste.get()} " MB"
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
                                    "Files smaller than this will be auto-pasted to clipboard"
                                </p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    "Background task threshold: " {move || bg_threshold.get()} "s"
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
                                    "Tasks longer than this will run in background"
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
                                    <div class="text-sm font-medium text-text-primary">"Smart Routing"</div>
                                    <div class="text-xs text-text-tertiary">
                                        "Auto-route requests to the most suitable provider"
                                    </div>
                                </div>
                            </label>
                        </div>

                        // Save feedback
                        {move || save_error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{e}</div>
                        })}
                        {move || save_success.get().then(|| view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">"Saved"</div>
                        })}

                        // Save button
                        <button
                            on:click=save
                            disabled=move || saving.get()
                            class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                        >
                            {move || if saving.get() { "Saving..." } else { "Save Settings" }}
                        </button>
                    </div>
                }.into_any()
            }
        }}
    }
}
