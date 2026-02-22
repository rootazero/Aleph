//! Providers Configuration View
//!
//! Provides UI for managing AI provider configurations:
//! - List all providers (preset + custom)
//! - Add/Edit/Delete providers
//! - Test connections
//! - Set default provider
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{ProvidersApi, ProviderInfo, ProviderConfig, TestResult};

#[component]
pub fn ProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let providers = RwSignal::new(Vec::<ProviderInfo>::new());
    let selected = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let saving = RwSignal::new(false);

    // Load providers on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match ProvidersApi::list(&state).await {
                    Ok(list) => {
                        providers.set(list);
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load providers: {}", e)));
                    }
                }
                loading.set(false);
            });
        }
    });

    // Subscribe to config events for real-time updates
    Effect::new(move || {
        if state.is_connected.get() {
            let _subscription_id = state.subscribe_events(move |event| {
                if event.topic.starts_with("config.providers") {
                    // Reload providers when config changes
                    spawn_local(async move {
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                    });
                }
            });
        }
    });

    view! {
        <div class="flex h-full">
            // Left sidebar: Provider list
            <ProviderList
                providers=providers
                selected=selected
                loading=loading
            />

            // Right panel: Provider editor
            <ProviderEditor
                providers=providers
                selected=selected
                saving=saving
                error=error
            />
        </div>
    }
}

// ============================================================================
// Provider List Component
// ============================================================================

#[component]
fn ProviderList(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    // Handle add new provider
    let on_add = move |_| {
        selected.set(Some("__new__".to_string()));
    };

    view! {
        <div class="w-80 border-r border-border bg-surface-raised flex flex-col">
            // Header
            <div class="p-4 border-b border-border">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-lg font-semibold text-text-primary">
                        "Providers"
                    </h2>
                    <button
                        on:click=on_add
                        class="px-3 py-1.5 bg-primary hover:bg-primary-hover text-white text-sm rounded-lg transition-colors"
                    >
                        "+ Add"
                    </button>
                </div>
                <p class="text-xs text-text-secondary">
                    "Manage AI provider configurations"
                </p>
            </div>

            // Provider list
            <div class="flex-1 overflow-y-auto p-4 space-y-2">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="text-center py-8 text-text-secondary text-sm">
                                "Loading providers..."
                            </div>
                        }.into_any()
                    } else if providers.get().is_empty() {
                        view! {
                            <div class="text-center py-8 text-text-tertiary text-sm">
                                "No providers configured"
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-2">
                                {move || providers.get().into_iter().map(|provider| {
                                    let name = provider.name.clone();
                                    let name_for_selected = name.clone();
                                    let name_for_click = name.clone();
                                    let is_selected = move || selected.get().as_ref() == Some(&name_for_selected);
                                    let on_click = move |_| {
                                        selected.set(Some(name_for_click.clone()));
                                    };

                                    view! {
                                        <ProviderCard
                                            provider=provider
                                            is_selected=Signal::derive(is_selected)
                                            on_click=on_click
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// Provider Card Component
// ============================================================================

#[component]
fn ProviderCard(
    provider: ProviderInfo,
    is_selected: Signal<bool>,
    on_click: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    let name = provider.name.clone();
    let model = provider.model.clone();
    let enabled = provider.enabled;
    let is_default = provider.is_default;

    view! {
        <button
            on:click=on_click
            class=move || {
                let base = "w-full text-left p-3 rounded-lg border transition-all";
                if is_selected.get() {
                    format!("{} bg-primary-subtle border-primary", base)
                } else {
                    format!("{} bg-surface-sunken border-border hover:bg-surface-sunken hover:border-border-strong", base)
                }
            }
        >
            <div class="flex items-start justify-between mb-1">
                <div class="flex items-center gap-2">
                    <span class="font-medium text-text-primary text-sm">
                        {name}
                    </span>
                    {move || if is_default {
                        view! {
                            <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded">
                                "Default"
                            </span>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                </div>
                <div class=move || {
                    if enabled {
                        "w-2 h-2 rounded-full bg-success"
                    } else {
                        "w-2 h-2 rounded-full bg-surface-sunken"
                    }
                }></div>
            </div>
            <div class="text-xs text-text-secondary truncate">
                {model}
            </div>
        </button>
    }
}

// ============================================================================
// Provider Editor Component
// ============================================================================

#[component]
fn ProviderEditor(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_protocol = RwSignal::new(String::from("openai"));
    let form_model = RwSignal::new(String::new());
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_color = RwSignal::new(String::from("#808080"));
    let form_timeout = RwSignal::new(300u64);
    let form_max_tokens = RwSignal::new(String::new());
    let form_temperature = RwSignal::new(String::new());

    // Test state
    let testing = RwSignal::new(false);
    let test_result = RwSignal::new(Option::<TestResult>::None);

    let is_new = move || selected.get().as_ref() == Some(&"__new__".to_string());
    let is_editing = move || selected.get().is_some();

    // Load provider data when selection changes
    Effect::new(move || {
        if let Some(name) = selected.get() {
            if name == "__new__" {
                // Reset form for new provider
                form_name.set(String::new());
                form_protocol.set(String::from("openai"));
                form_model.set(String::new());
                form_api_key.set(String::new());
                form_base_url.set(String::new());
                form_enabled.set(true);
                form_color.set(String::from("#808080"));
                form_timeout.set(300);
                form_max_tokens.set(String::new());
                form_temperature.set(String::new());
            } else {
                // Load existing provider
                if let Some(provider) = providers.get().iter().find(|p| p.name == name) {
                    form_name.set(provider.name.clone());
                    form_protocol.set(provider.provider_type.clone().unwrap_or_else(|| "openai".to_string()));
                    form_model.set(provider.model.clone());
                    form_enabled.set(provider.enabled);
                    // Note: We can't load api_key, base_url, etc. from ProviderInfo
                    // as they're not included in the list response for security
                }
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let name = form_name.get();
        if name.is_empty() {
            error.set(Some("Provider name is required".to_string()));
            return;
        }

        let model = form_model.get();
        if model.is_empty() {
            error.set(Some("Model name is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let config = ProviderConfig {
            protocol: Some(form_protocol.get()),
            enabled: form_enabled.get(),
            model: model.clone(),
            api_key: {
                let key = form_api_key.get();
                if key.is_empty() { None } else { Some(key) }
            },
            base_url: {
                let url = form_base_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            color: Some(form_color.get()),
            timeout_seconds: Some(form_timeout.get()),
            max_tokens: {
                let tokens = form_max_tokens.get();
                if tokens.is_empty() { None } else { tokens.parse().ok() }
            },
            temperature: {
                let temp = form_temperature.get();
                if temp.is_empty() { None } else { temp.parse().ok() }
            },
            top_p: None,
            top_k: None,
        };

        spawn_local(async move {
            let result = if is_new() {
                ProvidersApi::create(&state, name.clone(), config).await
            } else {
                ProvidersApi::update(&state, name.clone(), config).await
            };

            match result {
                Ok(()) => {
                    error.set(None);
                    // Reload providers list
                    if let Ok(list) = ProvidersApi::list(&state).await {
                        providers.set(list);
                    }
                    // Clear selection
                    selected.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    // Handle delete
    let on_delete = move |_| {
        if let Some(name) = selected.get() {
            if name == "__new__" {
                return;
            }

            saving.set(true);
            error.set(None);

            spawn_local(async move {
                match ProvidersApi::delete(&state, name.clone()).await {
                    Ok(()) => {
                        error.set(None);
                        // Reload providers list
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                        // Clear selection
                        selected.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to delete: {}", e)));
                    }
                }
                saving.set(false);
            });
        }
    };

    // Handle set default
    let on_set_default = move |_| {
        if let Some(name) = selected.get() {
            if name == "__new__" {
                return;
            }

            saving.set(true);
            error.set(None);

            spawn_local(async move {
                match ProvidersApi::set_default(&state, name.clone()).await {
                    Ok(()) => {
                        error.set(None);
                        // Reload providers list
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to set default: {}", e)));
                    }
                }
                saving.set(false);
            });
        }
    };

    // Handle test connection
    let on_test = move |_| {
        testing.set(true);
        test_result.set(None);
        error.set(None);

        let config = ProviderConfig {
            protocol: Some(form_protocol.get()),
            enabled: form_enabled.get(),
            model: form_model.get(),
            api_key: {
                let key = form_api_key.get();
                if key.is_empty() { None } else { Some(key) }
            },
            base_url: {
                let url = form_base_url.get();
                if url.is_empty() { None } else { Some(url) }
            },
            color: Some(form_color.get()),
            timeout_seconds: Some(form_timeout.get()),
            max_tokens: {
                let tokens = form_max_tokens.get();
                if tokens.is_empty() { None } else { tokens.parse().ok() }
            },
            temperature: {
                let temp = form_temperature.get();
                if temp.is_empty() { None } else { temp.parse().ok() }
            },
            top_p: None,
            top_k: None,
        };

        spawn_local(async move {
            match ProvidersApi::test_connection(&state, config).await {
                Ok(result) => {
                    test_result.set(Some(result));
                }
                Err(e) => {
                    error.set(Some(format!("Test failed: {}", e)));
                }
            }
            testing.set(false);
        });
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    view! {
                        <div class="flex items-center justify-center h-full text-text-tertiary">
                            "Select a provider to edit or add a new one"
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { "Add Provider" } else { "Edit Provider" }}
                                </h2>
                                <p class="text-sm text-text-secondary">
                                    "Configure AI provider settings"
                                </p>
                            </div>

                            // Error message
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="space-y-6">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Provider Name"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                        prop:disabled=move || !is_new()
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary disabled:opacity-50 disabled:cursor-not-allowed"
                                        placeholder="e.g., openai, claude, gemini"
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        "Unique identifier for this provider"
                                    </p>
                                </div>

                                // Protocol
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Protocol"
                                    </label>
                                    <select
                                        prop:value=move || form_protocol.get()
                                        on:change=move |ev| form_protocol.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                    >
                                        <option value="openai">"OpenAI"</option>
                                        <option value="anthropic">"Anthropic (Claude)"</option>
                                        <option value="gemini">"Google Gemini"</option>
                                        <option value="ollama">"Ollama"</option>
                                    </select>
                                </div>

                                // Model
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Model"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_model.get()
                                        on:input=move |ev| form_model.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="e.g., gpt-4o, claude-sonnet-4-5, gemini-3-flash"
                                    />
                                </div>

                                // API Key
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "API Key"
                                    </label>
                                    <input
                                        type="password"
                                        prop:value=move || form_api_key.get()
                                        on:input=move |ev| form_api_key.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="sk-..."
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        "Optional for local providers like Ollama"
                                    </p>
                                </div>

                                // Base URL
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Base URL"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_base_url.get()
                                        on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="https://api.openai.com/v1"
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        "Optional, defaults to official API endpoint"
                                    </p>
                                </div>

                                // Enabled toggle
                                <div class="flex items-center gap-3">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || form_enabled.get()
                                        on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                                        class="w-4 h-4 rounded border-border bg-surface-sunken text-primary focus:ring-primary/30"
                                    />
                                    <label class="text-sm font-medium text-text-secondary">
                                        "Enabled"
                                    </label>
                                </div>

                                // Advanced settings (collapsible)
                                <details class="group">
                                    <summary class="cursor-pointer text-sm font-medium text-text-secondary hover:text-text-primary">
                                        "Advanced Settings"
                                    </summary>
                                    <div class="mt-4 space-y-4 pl-4 border-l-2 border-border">
                                        // Timeout
                                        <div>
                                            <label class="block text-sm font-medium text-text-secondary mb-2">
                                                "Timeout (seconds)"
                                            </label>
                                            <input
                                                type="number"
                                                prop:value=move || form_timeout.get()
                                                on:input=move |ev| {
                                                    if let Ok(val) = event_target_value(&ev).parse() {
                                                        form_timeout.set(val);
                                                    }
                                                }
                                                class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            />
                                        </div>

                                        // Max Tokens
                                        <div>
                                            <label class="block text-sm font-medium text-text-secondary mb-2">
                                                "Max Tokens"
                                            </label>
                                            <input
                                                type="number"
                                                prop:value=move || form_max_tokens.get()
                                                on:input=move |ev| form_max_tokens.set(event_target_value(&ev))
                                                class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                placeholder="Optional"
                                            />
                                        </div>

                                        // Temperature
                                        <div>
                                            <label class="block text-sm font-medium text-text-secondary mb-2">
                                                "Temperature"
                                            </label>
                                            <input
                                                type="number"
                                                step="0.1"
                                                prop:value=move || form_temperature.get()
                                                on:input=move |ev| form_temperature.set(event_target_value(&ev))
                                                class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                placeholder="0.0 - 2.0"
                                            />
                                        </div>
                                    </div>
                                </details>
                            </div>

                            // Actions
                            <div class="mt-8 flex items-center gap-3">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </button>

                                <button
                                    on:click=on_test
                                    prop:disabled=move || testing.get() || saving.get()
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <>
                                                <button
                                                    on:click=on_set_default
                                                    prop:disabled=move || saving.get()
                                                    class="px-6 py-2 bg-success hover:bg-success disabled:bg-success/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                                >
                                                    "Set as Default"
                                                </button>

                                                <button
                                                    on:click=on_delete
                                                    prop:disabled=move || saving.get()
                                                    class="px-6 py-2 bg-danger hover:bg-danger disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                                >
                                                    "Delete"
                                                </button>
                                            </>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-sunken text-text-primary rounded-lg transition-colors"
                                >
                                    "Cancel"
                                </button>
                            </div>

                            // Test result display
                            {move || {
                                if let Some(result) = test_result.get() {
                                    if result.success {
                                        view! {
                                            <div class="mt-4 p-4 bg-success-subtle border border-success/20 rounded-lg">
                                                <div class="flex items-center gap-2 text-success">
                                                    <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"></path>
                                                    </svg>
                                                    <span class="font-medium">"Connection successful!"</span>
                                                </div>
                                                {move || {
                                                    if let Some(latency) = result.latency_ms {
                                                        view! {
                                                            <p class="mt-1 text-sm text-success">
                                                                {format!("Latency: {}ms", latency)}
                                                            </p>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }
                                                }}
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="mt-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg">
                                                <div class="flex items-center gap-2 text-danger">
                                                    <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"></path>
                                                    </svg>
                                                    <span class="font-medium">"Connection failed"</span>
                                                </div>
                                                {move || {
                                                    if let Some(err) = result.error.clone() {
                                                        view! {
                                                            <p class="mt-1 text-sm text-danger">
                                                                {err}
                                                            </p>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }
                                                }}
                                            </div>
                                        }.into_any()
                                    }
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

