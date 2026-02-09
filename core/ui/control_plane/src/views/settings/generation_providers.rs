use leptos::*;
use leptos::prelude::*;
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
        <div class="flex flex-col h-full">
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
            <div class="flex-1 overflow-hidden">
                {move || {
                    if is_loading.get() {
                        view! {
                            <div class="flex items-center justify-center h-full">
                                <div class="text-gray-500">"Loading..."</div>
                            </div>
                        }.into_view()
                    } else if let Some(error) = error_message.get() {
                        view! {
                            <div class="flex items-center justify-center h-full">
                                <div class="text-red-500">{error}</div>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 p-6">
                                <For
                                    each=current_presets
                                    key=|preset| preset.id.clone()
                                    children=move |preset: PresetProvider| {
                                        let preset_id = preset.id.clone();
                                        let configured = is_configured(&preset_id);
                                        let entry = get_provider_entry(&preset_id);

                                        view! {
                                            <ProviderCard
                                                preset=preset
                                                is_configured=configured
                                                entry=entry
                                                on_click=move |_| {
                                                    set_selected_provider_id.set(Some(preset_id.clone()));
                                                }
                                            />
                                        }
                                    }
                                />
                            </div>
                        }.into_view()
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
            class="border border-gray-200 dark:border-gray-700 rounded-lg p-4 hover:border-blue-500 cursor-pointer transition-colors"
            class:opacity-50=preset.is_unsupported
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
