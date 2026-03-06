use leptos::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::{
    EmbeddingProvidersApi, EmbeddingProviderEntry, EmbeddingProviderConfig,
    EmbeddingPresetEntry,
};
use crate::context::DashboardState;

#[component]
pub fn EmbeddingProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State signals
    let (providers, set_providers) = signal(Vec::<EmbeddingProviderEntry>::new());
    let (presets, set_presets) = signal(Vec::<EmbeddingPresetEntry>::new());
    let (is_loading, set_is_loading) = signal(true);
    let (error_message, set_error_message) = signal(Option::<String>::None);
    let (selected_provider_id, set_selected_provider_id) = signal(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);

    // Load providers and presets on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                set_is_loading.set(true);
                let providers_result = EmbeddingProvidersApi::list(&state).await;
                let presets_result = EmbeddingProvidersApi::presets(&state).await;

                match (providers_result, presets_result) {
                    (Ok(list), Ok(preset_list)) => {
                        // Auto-select the active provider on first load
                        if selected_provider_id.get_untracked().is_none() {
                            if let Some(active) = list.iter().find(|p| p.is_active) {
                                set_selected_provider_id.set(Some(active.id.clone()));
                            }
                        }
                        set_providers.set(list);
                        set_presets.set(preset_list);
                        set_is_loading.set(false);
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        set_error_message.set(Some(format!("Failed to load: {}", e)));
                        set_is_loading.set(false);
                    }
                }
            });
        } else {
            set_is_loading.set(false);
        }
    });

    // Reload helper
    let reload = move || {
        spawn_local(async move {
            if let Ok(list) = EmbeddingProvidersApi::list(&state).await {
                set_providers.set(list);
            }
        });
    };

    view! {
        <div class="flex h-full">
            // Left panel - Provider list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
                // Header
                <div class="px-6 py-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        "Embedding Providers"
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        "Configure vector embedding providers for memory and knowledge base"
                    </p>
                </div>

                // Content
                <div class="flex-1 overflow-auto">
                    {move || {
                        if is_loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">"Loading embedding providers..."</div>
                                </div>
                            }.into_any()
                        } else if let Some(error) = error_message.get() {
                            view! {
                                <div class="p-6">
                                    <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{error}</div>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="p-6 space-y-4">
                                    // Preset Grid
                                    <div>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                            "Embedding Providers"
                                        </h2>
                                        <div class="grid grid-cols-1 gap-2">
                                            {move || {
                                                let preset_list = presets.get();
                                                let provider_list = providers.get();
                                                preset_list.into_iter().map(|preset| {
                                                    let preset_id = preset.id.clone();
                                                    let preset_name = preset.name.clone();
                                                    let preset_label = preset.preset.clone();
                                                    let model = preset.model.clone();
                                                    let dims = preset.dimensions;
                                                    let first_char = preset_name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                                    // Check if this preset is configured
                                                    let configured_provider = provider_list.iter().find(|p| p.preset == preset_label);
                                                    let is_configured = configured_provider.is_some();
                                                    let is_active = configured_provider.map_or(false, |p| p.is_active);
                                                    let is_verified = configured_provider.map_or(false, |p| p.verified);
                                                    let configured_id = configured_provider.map(|p| p.id.clone());

                                                    let sel_id = configured_id.clone().unwrap_or(preset_id.clone());
                                                    let sel_id_click = sel_id.clone();
                                                    let sel_id_check = sel_id.clone();

                                                    let icon_color = match preset_label.as_str() {
                                                        "silicon_flow" => "#6C5CE7",
                                                        "open_ai" => "#10A37F",
                                                        "ollama" => "#1D1D1F",
                                                        _ => "#808080",
                                                    };

                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                set_selected_provider_id.set(Some(sel_id_click.clone()));
                                                                set_show_add_form.set(false);
                                                            }
                                                            class=move || {
                                                                let base = "text-left p-3 rounded-lg border transition-all";
                                                                let is_sel = selected_provider_id.get().as_deref() == Some(&sel_id_check);
                                                                if is_sel {
                                                                    format!("{} bg-primary-subtle border-primary", base)
                                                                } else if is_configured {
                                                                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                                                } else {
                                                                    format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                                                                }
                                                            }
                                                        >
                                                            <div class="flex items-center gap-3">
                                                                <div
                                                                    class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                                    style=format!("background-color: {}", icon_color)
                                                                >
                                                                    {first_char}
                                                                </div>
                                                                <div class="min-w-0">
                                                                    <div class="flex items-center gap-2">
                                                                        <span class="font-medium text-text-primary text-sm truncate">
                                                                            {preset_name}
                                                                        </span>
                                                                        {if is_active {
                                                                            view! {
                                                                                <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                                                    "Default"
                                                                                </span>
                                                                            }.into_any()
                                                                        } else if is_verified {
                                                                            view! {
                                                                                <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                                                    "Verified"
                                                                                </span>
                                                                            }.into_any()
                                                                        } else {
                                                                            view! { <span></span> }.into_any()
                                                                        }}
                                                                    </div>
                                                                    <div class="text-xs text-text-tertiary truncate">
                                                                        {format!("{} · {}d", model, dims)}
                                                                    </div>
                                                                </div>
                                                            </div>
                                                        </button>
                                                    }
                                                }).collect_view()
                                            }}
                                        </div>
                                    </div>

                                    // Custom providers (not matching any preset)
                                    {move || {
                                        let provider_list = providers.get();
                                        let preset_labels: Vec<String> = presets.get().iter().map(|p| p.preset.clone()).collect();
                                        let custom_providers: Vec<_> = provider_list.into_iter()
                                            .filter(|p| !preset_labels.contains(&p.preset))
                                            .collect();
                                        if custom_providers.is_empty() {
                                            view! { <div></div> }.into_any()
                                        } else {
                                            view! {
                                                <div class="pt-2">
                                                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                        "Custom Providers"
                                                    </h2>
                                                    <div class="grid grid-cols-1 gap-2">
                                                        {custom_providers.into_iter().map(|cp| {
                                                            let cp_id = cp.id.clone();
                                                            let cp_name = cp.name.clone();
                                                            let cp_model = cp.model.clone();
                                                            let cp_dims = cp.dimensions;
                                                            let cp_is_active = cp.is_active;
                                                            let cp_verified = cp.verified;
                                                            let first_char = cp_name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                                            let sel_id = cp_id.clone();
                                                            let sel_id_check = cp_id.clone();

                                                            view! {
                                                                <button
                                                                    on:click=move |_| {
                                                                        set_selected_provider_id.set(Some(sel_id.clone()));
                                                                        set_show_add_form.set(false);
                                                                    }
                                                                    class=move || {
                                                                        let base = "text-left p-3 rounded-lg border transition-all";
                                                                        let is_sel = selected_provider_id.get().as_deref() == Some(&sel_id_check);
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
                                                                            style="background-color: #808080"
                                                                        >
                                                                            {first_char}
                                                                        </div>
                                                                        <div class="min-w-0">
                                                                            <div class="flex items-center gap-2">
                                                                                <span class="font-medium text-text-primary text-sm truncate">
                                                                                    {cp_name}
                                                                                </span>
                                                                                {if cp_is_active {
                                                                                    view! {
                                                                                        <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                                                            "Default"
                                                                                        </span>
                                                                                    }.into_any()
                                                                                } else if cp_verified {
                                                                                    view! {
                                                                                        <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                                                            "Verified"
                                                                                        </span>
                                                                                    }.into_any()
                                                                                } else {
                                                                                    view! { <span></span> }.into_any()
                                                                                }}
                                                                            </div>
                                                                            <div class="text-xs text-text-tertiary truncate">
                                                                                {format!("{} · {}d", cp_model, cp_dims)}
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
                </div>
            </div>

            // Right panel - Detail / Add form
            <div class="w-7/12 min-w-[320px] bg-surface">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddProviderPanel
                                on_added=move || {
                                    set_show_add_form.set(false);
                                    reload();
                                }
                                on_cancel=move || set_show_add_form.set(false)
                            />
                        }.into_any()
                    } else if let Some(provider_id) = selected_provider_id.get() {
                        let provider = providers.get().into_iter().find(|p| p.id == provider_id);
                        if let Some(provider) = provider {
                            view! {
                                <ProviderDetailPanel
                                    provider=provider
                                    on_reload=move || reload()
                                />
                            }.into_any()
                        } else {
                            view! { <EmptyState /> }.into_any()
                        }
                    } else {
                        view! { <EmptyState /> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// Empty State
// ============================================================================

#[component]
fn EmptyState() -> impl IntoView {
    view! {
        <div class="flex items-center justify-center h-full">
            <div class="text-center text-text-secondary">
                <p class="text-lg">"Select a provider to view details"</p>
                <p class="text-sm text-text-tertiary mt-1">"or add a new embedding provider"</p>
            </div>
        </div>
    }
}

// ============================================================================
// Provider Detail Panel
// ============================================================================

#[component]
fn ProviderDetailPanel(
    provider: EmbeddingProviderEntry,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let provider_id = provider.id.clone();
    let is_active = provider.is_active;
    let is_custom = provider.preset == "custom";

    // Clone fields needed in multiple closures and the view
    let provider_name = provider.name.clone();
    let provider_preset = provider.preset.clone();
    let provider_api_key_env = provider.api_key_env.clone();
    let provider_batch_size = provider.batch_size;
    let provider_timeout_ms = provider.timeout_ms;

    // Editable fields
    let api_base = RwSignal::new(provider.api_base.clone());
    let api_key = RwSignal::new(provider.api_key.clone().unwrap_or_default());
    let model = RwSignal::new(provider.model.clone());
    let dimensions = RwSignal::new(provider.dimensions);

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
                    if key.is_empty() { None } else { Some(key) }
                },
                model: model.get(),
                dimensions: dimensions.get(),
                batch_size: provider_batch_size,
                timeout_ms: provider_timeout_ms,
            }
        }
    };

    // Test connection handler
    let build_config_for_test = build_config.clone();
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let config = build_config_for_test();

        spawn_local(async move {
            match EmbeddingProvidersApi::test(&state, config).await {
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

    // Save handler
    let build_config_for_save = build_config.clone();
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
                    set_timeout(move || set_save_success.set(false), std::time::Duration::from_secs(2));
                }
                Err(e) => {
                    set_saving.set(false);
                    set_action_error.set(Some(format!("Save failed: {}", e)));
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
                    set_action_error.set(Some(format!("Activation failed: {}", e)));
                }
            }
        });
    };

    // Delete handler
    let provider_id_for_delete = provider_id.clone();
    let handle_delete = move |_| {
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
                    set_action_error.set(Some(format!("Delete failed: {}", e)));
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
                            {provider_name.clone()}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {format!("ID: {}", provider_id.clone())}
                        </p>
                    </div>
                    <div class="flex gap-1">
                        {if is_active {
                            view! {
                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-primary-subtle text-primary">
                                    "Default"
                                </span>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }}
                        {if provider.verified {
                            view! {
                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-success-subtle text-success">
                                    "Verified"
                                </span>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }}
                    </div>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "API Base URL"
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder={
                            let default_base = match provider_preset.as_str() {
                                "silicon_flow" => "Default: https://api.siliconflow.cn/v1",
                                "open_ai" => "Default: https://api.openai.com/v1",
                                "ollama" => "Default: http://localhost:11434/v1",
                                _ => "https://api.example.com/v1",
                            };
                            default_base
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "API Key"
                    </label>
                    <input
                        type="password"
                        value=move || api_key.get()
                        on:input=move |ev| api_key.set(event_target_value(&ev))
                        placeholder="Enter API key (leave blank to use env var)"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    {provider_api_key_env.clone().map(|env_var| view! {
                        <p class="mt-1 text-xs text-text-tertiary">
                            {format!("Env var: {}", env_var)}
                        </p>
                    })}
                </div>

                // Model Name
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Model"
                    </label>
                    <input
                        type="text"
                        value=move || model.get()
                        on:input=move |ev| model.set(event_target_value(&ev))
                        placeholder={
                            let default_model = match provider_preset.as_str() {
                                "silicon_flow" => "Default: BAAI/bge-m3",
                                "open_ai" => "Default: text-embedding-3-small",
                                "ollama" => "Default: nomic-embed-text",
                                _ => "model-name",
                            };
                            default_model
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Dimensions
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Dimensions"
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
                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                </button>

                <button
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>

            // Actions — Row 2: Set as Default + Delete (only for existing providers)
            {if !is_active || is_custom {
                Some(view! {
                    <div class="flex flex-row gap-3">
                        {if !is_active {
                            Some(view! {
                                <button
                                    on:click=handle_activate
                                    disabled=move || activating.get()
                                    class="flex-1 px-4 py-2.5 bg-success-subtle border border-success/20 text-success rounded-lg hover:bg-success-subtle/80 disabled:opacity-50 transition-colors font-medium"
                                >
                                    {move || if activating.get() { "Setting default..." } else { "Set as Default" }}
                                </button>
                            })
                        } else {
                            None
                        }}
                        {if is_custom {
                            Some(view! {
                                <button
                                    on:click=handle_delete
                                    disabled=move || deleting.get()
                                    class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50 transition-colors font-medium"
                                >
                                    {move || if deleting.get() { "Deleting..." } else { "Delete" }}
                                </button>
                            })
                        } else {
                            None
                        }}
                    </div>
                })
            } else {
                None
            }}

            </div> // scrollable content
        </div> // flex wrapper
    }
}

// ============================================================================
// Add Provider Panel
// ============================================================================

#[component]
fn AddProviderPanel(
    on_added: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form state — custom provider only
    let id = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let api_base = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let model_name = RwSignal::new(String::new());
    let dimensions = RwSignal::new(1024u32);

    let (adding, set_adding) = signal(false);
    let (testing, set_testing) = signal(false);
    let (add_error, set_add_error) = signal(Option::<String>::None);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);

    // Build config from form
    let build_config = move || -> EmbeddingProviderConfig {
        EmbeddingProviderConfig {
            id: id.get(),
            name: name.get(),
            preset: "custom".to_string(),
            api_base: api_base.get(),
            api_key_env: None,
            api_key: {
                let key = api_key.get();
                if key.is_empty() { None } else { Some(key) }
            },
            model: model_name.get(),
            dimensions: dimensions.get(),
            batch_size: 32,
            timeout_ms: 10000,
        }
    };

    // Test handler
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_add_error.set(None);

        let config = build_config();

        spawn_local(async move {
            match EmbeddingProvidersApi::test(&state, config).await {
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

    // Add handler
    let handle_add = move |_| {
        set_adding.set(true);
        set_add_error.set(None);

        let config = build_config();

        if config.id.is_empty() || config.name.is_empty() {
            set_add_error.set(Some("ID and Name are required".to_string()));
            set_adding.set(false);
            return;
        }

        spawn_local(async move {
            match EmbeddingProvidersApi::add(&state, config).await {
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
                // ID
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Provider ID"</label>
                    <input
                        type="text"
                        value=move || id.get()
                        on:input=move |ev| id.set(event_target_value(&ev))
                        placeholder="e.g., my-openai"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">"Unique identifier (lowercase, no spaces)"</p>
                </div>

                // Name
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Display Name"</label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder="e.g., My OpenAI Embedding"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"API Base URL"</label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder="https://api.openai.com/v1"
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

                // Model
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Model"</label>
                    <input
                        type="text"
                        value=move || model_name.get()
                        on:input=move |ev| model_name.set(event_target_value(&ev))
                        placeholder="text-embedding-3-small"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Dimensions
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Dimensions"</label>
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
                    <p class="mt-1 text-xs text-text-tertiary">"Output vector dimensions of the model"</p>
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
