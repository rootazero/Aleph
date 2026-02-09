use leptos::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;
use crate::api::{GenerationProvidersApi, GenerationProviderConfig, GenerationProviderEntry};
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
    let (is_loading, set_is_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(Option::<String>::None);

    // Load providers on mount
    create_effect(move |_| {
        if state.is_connected.get() {
            spawn_local(async move {
                set_is_loading.set(true);
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
        }
    });

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
            // Left panel - Provider list
            <div class="flex flex-col w-2/3 border-r border-gray-200 dark:border-gray-700">
                // Header
                <div class="px-6 py-4 border-b border-gray-200 dark:border-gray-700">
                    <h1 class="text-2xl font-semibold text-gray-900 dark:text-gray-100">
                        "Generation Providers"
                    </h1>
                    <p class="mt-1 text-sm text-gray-600 dark:text-gray-400">
                        "Configure image, video, and audio generation providers"
                    </p>
                </div>

                // Category Tabs
                <div class="px-6 py-3 border-b border-gray-200 dark:border-gray-700">
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
                {move || {
                    if is_loading.get() {
                        view! {
                            <div class="flex items-center justify-center h-full">
                                <div class="text-gray-500">"Loading..."</div>
                            </div>
                        }.into_any()
                    } else if let Some(error) = error_message.get() {
                        view! {
                            <div class="flex items-center justify-center h-full">
                                <div class="text-red-500">{error}</div>
                            </div>
                        }.into_any()
                    } else {
                        let presets = current_presets();
                        view! {
                            <div class="grid grid-cols-1 md:grid-cols-2 gap-4 p-6">
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
                                            }
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
                </div>
            </div>

            // Right panel - Provider details
            <div class="flex-1 bg-gray-50 dark:bg-gray-900">
                <ProviderDetailPanel
                    selected_id=selected_provider_id
                    providers=providers
                    on_reload=move || {
                        spawn_local(async move {
                            if let Ok(list) = GenerationProvidersApi::list(&state).await {
                                set_providers.set(list);
                            }
                        });
                    }
                />
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
                    format!("{} bg-blue-500 text-white", base)
                } else {
                    format!("{} bg-gray-100 dark:bg-gray-800 text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-700", base)
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

    view! {
        <div
            class=move || {
                let base = "border rounded-lg p-4 hover:border-blue-500 cursor-pointer transition-colors";
                let selected = if is_selected { " ring-2 ring-blue-500 border-blue-500 bg-blue-50 dark:bg-blue-900/20" } else { " border-gray-200 dark:border-gray-700" };
                let opacity = if preset.is_unsupported { " opacity-50" } else { "" };
                format!("{}{}{}", base, selected, opacity)
            }
            on:click=on_click
        >
            <div class="flex items-start justify-between mb-3">
                <div class="flex items-center gap-2">
                    <span class="text-2xl">{preset.icon.clone()}</span>
                    <div>
                        <h3 class="font-semibold text-gray-900 dark:text-gray-100">
                            {preset.name.clone()}
                        </h3>
                        {preset.is_unsupported.then(|| view! {
                            <span class="text-xs text-gray-500">"(Unsupported)"</span>
                        })}
                    </div>
                </div>
                {move || {
                    if is_configured {
                        if is_default() {
                            view! {
                                <span class="px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200 rounded">
                                    "Default"
                                </span>
                            }.into_view()
                        } else {
                            view! {
                                <span class="px-2 py-1 text-xs font-medium bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200 rounded">
                                    "Configured"
                                </span>
                            }.into_view()
                        }
                    } else {
                        view! {
                            <span class="px-2 py-1 text-xs font-medium bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400 rounded">
                                "Not configured"
                            </span>
                        }.into_view()
                    }
                }}
            </div>

            <p class="text-sm text-gray-600 dark:text-gray-400 mb-3">
                {preset.description.clone()}
            </p>

            <div class="flex items-center gap-2 text-xs text-gray-500">
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
        <div class="h-full overflow-auto">
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
        <div class="flex items-center justify-center h-full">
            <div class="text-center text-gray-500 dark:text-gray-400">
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
        <div class="p-6 space-y-6">
            // Header
            <div class="flex items-center justify-between">
                <h2 class="text-xl font-semibold text-gray-900 dark:text-gray-100">
                    {provider.name.clone()}
                </h2>
            </div>

            // Provider details
            <div class="space-y-4">
                <DetailField label="Provider Type" value=provider.config.provider_type.clone() />
                <DetailField label="Model" value=provider.config.model.clone().unwrap_or_else(|| "N/A".to_string()) />
                <DetailField label="Base URL" value=provider.config.base_url.clone().unwrap_or_else(|| "N/A".to_string()) />
                <DetailField label="API Key" value=if provider.config.api_key.is_some() { "••••••••".to_string() } else { "Not set".to_string() } />
                <DetailField label="Enabled" value=if provider.config.enabled { "Yes" } else { "No" }.to_string() />
                <DetailField label="Timeout" value=format!("{} seconds", provider.config.timeout_seconds) />
                <DetailField label="Capabilities" value=capabilities_str.clone() />
            </div>

            // Default status
            {move || {
                if has_defaults {
                    view! {
                        <div class="p-3 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded">
                            <p class="text-sm text-blue-800 dark:text-blue-200">
                                "This is the default provider for: "
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
                            <div class="p-3 bg-green-50 dark:bg-green-900/20 border border-green-200 dark:border-green-800 rounded">
                                <p class="text-sm text-green-800 dark:text-green-200">
                                    "✓ " {message}
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
                                <p class="text-sm text-red-800 dark:text-red-200">
                                    "✗ " {message}
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
                <div class="p-3 bg-red-50 border border-red-200 rounded text-red-700 text-sm">
                    {e}
                </div>
            })}

            // Actions
            <div class="space-y-3 pt-4 border-t border-gray-200 dark:border-gray-700">
                // Test connection
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="w-full px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 disabled:opacity-50 transition-colors"
                >
                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                </button>

                // Set as default buttons
                <div class="space-y-2">
                    <p class="text-sm font-medium text-gray-700 dark:text-gray-300">"Set as default for:"</p>
                    {capabilities.iter().map(|cap| {
                        let gen_type = *cap;
                        let is_default = is_default_for.contains(&gen_type);
                        let set_default = handle_set_default.clone();

                        view! {
                            <button
                                on:click=move |_| set_default(gen_type)
                                disabled=move || setting_default.get() || is_default
                                class=move || {
                                    let base = "w-full px-4 py-2 rounded-lg transition-colors";
                                    if is_default {
                                        format!("{} bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200 cursor-not-allowed", base)
                                    } else {
                                        format!("{} bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700 disabled:opacity-50", base)
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
                    class="w-full px-4 py-2 bg-red-50 text-red-600 rounded-lg hover:bg-red-100 disabled:opacity-50 transition-colors"
                >
                    {move || if deleting.get() { "Deleting..." } else { "Delete Provider" }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn DetailField(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div>
            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                {label}
            </label>
            <div class="text-gray-900 dark:text-gray-100">
                {value}
            </div>
        </div>
    }
}
