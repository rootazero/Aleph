use leptos::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;
use crate::api::{GenerationProvidersApi, GenerationProviderConfig, GenerationProviderEntry};
use crate::api::{GenerationConfig, GenerationConfigApi};
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::preset_providers::{PresetProvider, PresetProviders};

#[component]
pub fn GenerationProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let (providers, set_providers) = create_signal(Vec::<GenerationProviderEntry>::new());
    let (selected_category, set_selected_category) = create_signal(GenerationType::Image);
    let (selected_provider_id, set_selected_provider_id) = create_signal(Option::<String>::None);
    let (show_add_form, set_show_add_form) = create_signal(false);
    let (is_loading, set_is_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(Option::<String>::None);

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
                                            let is_selected = selected_provider_id.get() == Some(preset_id.clone());

                                            view! {
                                                <ProviderCard
                                                    preset=preset
                                                    is_configured=configured
                                                    entry=entry
                                                    is_selected=is_selected
                                                    on_click=move |_| {
                                                        set_selected_provider_id.set(Some(preset_id.clone()));
                                                        set_show_add_form.set(false);
                                                    }
                                                />
                                            }
                                        }).collect_view()}
                                    </div>

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
    let is_default = move || {
        if let Some(ref e) = entry {
            !e.is_default_for.is_empty()
        } else {
            false
        }
    };

    let is_verified = entry.as_ref().map_or(false, |e| e.config.verified);

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
                                        }.into_view()
                                    } else {
                                        view! { <span></span> }.into_view()
                                    }}
                                </div>
                            }.into_view()
                        } else if is_verified {
                            view! {
                                <span class="px-2 py-1 text-xs font-medium bg-success-subtle text-success rounded">
                                    "Active"
                                </span>
                            }.into_view()
                        } else {
                            view! { <span></span> }.into_view()
                        }
                    } else {
                        view! { <span></span> }.into_view()
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
    let state = expect_context::<DashboardState>();

    view! {
        <div class="h-full">
            {move || {
                if let Some(provider_id) = selected_id.get() {
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

    // Clone values needed by closures
    let provider_name_for_delete = provider.name.clone();
    let provider_name_for_default = provider.name.clone();
    let provider_type = provider.config.provider_type.clone();
    let api_key = provider.config.api_key.clone();
    let base_url = provider.config.base_url.clone();
    let model = provider.config.model.clone();
    let capabilities = provider.config.capabilities.clone();
    let is_default_for = provider.is_default_for.clone();

    // Pre-compute values outside view!
    let capabilities_str = capabilities.iter()
        .map(|c| c.display_name())
        .collect::<Vec<_>>()
        .join(", ");

    let default_for_str = is_default_for.iter()
        .map(|t| t.display_name())
        .collect::<Vec<_>>()
        .join(", ");

    let has_defaults = !is_default_for.is_empty();

    // State for actions
    let (deleting, set_deleting) = create_signal(false);
    let (testing, set_testing) = create_signal(false);
    let (setting_default, set_setting_default) = create_signal(false);
    let (action_error, set_action_error) = create_signal(Option::<String>::None);
    let (test_result, set_test_result) = create_signal(Option::<(bool, String)>::None);

    // Delete handler
    let handle_delete = move |_| {
        let name = provider_name_for_delete.clone();
        set_deleting.set(true);
        set_action_error.set(None);

        spawn_local(async move {
            match GenerationProvidersApi::delete(&state, &name).await {
                Ok(_) => {
                    set_deleting.set(false);
                    on_reload();
                }
                Err(e) => {
                    set_deleting.set(false);
                    set_action_error.set(Some(format!("Delete failed: {}", e)));
                }
            }
        });
    };

    // Test connection handler
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let ptype = provider_type.clone();
        let key = api_key.clone();
        let url = base_url.clone();
        let mdl = model.clone();

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

    // Set default handler - wrap in Rc for cloning
    let handle_set_default = Rc::new({
        let name = provider_name_for_default.clone();
        move |gen_type: GenerationType| {
            let name = name.clone();
            set_setting_default.set(true);
            set_action_error.set(None);

            spawn_local(async move {
                match GenerationProvidersApi::set_default(&state, &name, gen_type).await {
                    Ok(_) => {
                        set_setting_default.set(false);
                        on_reload();
                    }
                    Err(e) => {
                        set_setting_default.set(false);
                        set_action_error.set(Some(format!("Set default failed: {}", e)));
                    }
                }
            });
        }
    });

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
                            {provider.config.provider_type.clone()}
                        </p>
                    </div>
                    <span class=move || {
                        if provider.config.enabled {
                            "px-2.5 py-1 rounded-full text-xs font-medium bg-success-subtle text-success"
                        } else {
                            "px-2.5 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary"
                        }
                    }>
                        {if provider.config.enabled { "Enabled" } else { "Disabled" }}
                    </span>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>
                <DetailField label="Model" value=provider.config.model.clone().unwrap_or_else(|| "N/A".to_string()) />
                <DetailField label="Base URL" value=provider.config.base_url.clone().unwrap_or_else(|| "N/A".to_string()) />
                <DetailField label="API Key" value=if provider.config.api_key.is_some() { "••••••••".to_string() } else { "Not set".to_string() } />
                <DetailField label="Timeout" value=format!("{} seconds", provider.config.timeout_seconds) />
                <DetailField label="Capabilities" value=capabilities_str.clone() />
            </div>

            // Default status
            {move || {
                if has_defaults {
                    view! {
                        <div class="p-3 bg-primary-subtle border border-primary/20 rounded-lg">
                            <p class="text-sm text-primary">
                                "Default provider for: "
                                {default_for_str.clone()}
                            </p>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Test result
            {move || {
                if let Some((success, message)) = test_result.get() {
                    if success {
                        view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded-lg">
                                <p class="text-sm text-success">
                                    {message}
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg">
                                <p class="text-sm text-danger">
                                    {message}
                                </p>
                            </div>
                        }.into_any()
                    }
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                    {e}
                </div>
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
                                let base = "w-full px-4 py-2.5 rounded-lg transition-colors font-medium";
                                if is_default {
                                    format!("{} bg-primary-subtle text-primary cursor-not-allowed", base)
                                } else {
                                    format!("{} bg-surface-sunken text-text-secondary hover:bg-surface-raised disabled:opacity-50", base)
                                }
                            }
                        >
                            {gen_type.icon()} " " {gen_type.display_name()}
                            {if is_default { " (Current)" } else { "" }}
                        </button>
                    }
                }).collect_view()}
            </div>

            // Delete button
            <button
                on:click=handle_delete
                disabled=move || deleting.get()
                class="w-full px-4 py-2.5 bg-danger-subtle text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors font-medium"
            >
                {move || if deleting.get() { "Deleting..." } else { "Delete Provider" }}
            </button>

            </div> // scrollable content
        </div> // flex wrapper
    }
}

#[component]
fn DetailField(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
                {label}
            </label>
            <div class="text-text-primary">
                {value}
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
    let model = RwSignal::new(String::new());
    let timeout = RwSignal::new(60u64);

    // Capability checkboxes
    let cap_image = RwSignal::new(true);
    let cap_video = RwSignal::new(false);
    let cap_audio = RwSignal::new(false);
    let cap_speech = RwSignal::new(false);

    let (adding, set_adding) = create_signal(false);
    let (testing, set_testing) = create_signal(false);
    let (add_error, set_add_error) = create_signal(Option::<String>::None);
    let (test_result, set_test_result) = create_signal(Option::<(bool, String)>::None);

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
            model: {
                let m = model.get();
                if m.is_empty() { None } else { Some(m) }
            },
            enabled: true,
            color: "#808080".to_string(),
            capabilities: build_capabilities(),
            timeout_seconds: timeout.get(),
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
                    <input
                        type="password"
                        value=move || api_key.get()
                        on:input=move |ev| api_key.set(event_target_value(&ev))
                        placeholder="sk-..."
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Base URL"</label>
                    <input
                        type="text"
                        value=move || base_url.get()
                        on:input=move |ev| base_url.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Model"</label>
                    <input
                        type="text"
                        value=move || model.get()
                        on:input=move |ev| model.set(event_target_value(&ev))
                        placeholder="e.g., dall-e-3"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
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
