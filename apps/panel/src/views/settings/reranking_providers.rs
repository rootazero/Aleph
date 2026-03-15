use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{RerankConfigApi, RerankConfig, RerankProviderType, ProvidersApi};
use crate::components::model_selector::{ModelSelector, ModelOption};
use crate::components::ui::SecretInput;
use crate::context::DashboardState;

/// Preset reranking provider metadata
struct RerankPreset {
    key: &'static str,
    name: &'static str,
    icon_color: &'static str,
    default_api_base: &'static str,
    default_model: &'static str,
}

const RERANK_PRESETS: &[RerankPreset] = &[
    RerankPreset { key: "jina",       name: "Jina AI",     icon_color: "#FF6B6B", default_api_base: "https://api.jina.ai/v1",          default_model: "jina-reranker-v2-base-multilingual" },
    RerankPreset { key: "siliconflow", name: "SiliconFlow", icon_color: "#6C5CE7", default_api_base: "https://api.siliconflow.cn/v1",   default_model: "BAAI/bge-reranker-v2-m3" },
    RerankPreset { key: "voyage",     name: "Voyage AI",   icon_color: "#00B4D8", default_api_base: "https://api.voyageai.com/v1",      default_model: "rerank-2" },
    RerankPreset { key: "pinecone",   name: "Pinecone",    icon_color: "#1DB954", default_api_base: "https://api.pinecone.io",          default_model: "pinecone-rerank-v0" },
    RerankPreset { key: "vllm",       name: "vLLM",        icon_color: "#FF9F1C", default_api_base: "http://localhost:8000/v1",         default_model: "BAAI/bge-reranker-v2-m3" },
];

#[component]
pub fn RerankingProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(Option::<RerankConfig>::None);
    let loading = RwSignal::new(true);
    let selected_provider = RwSignal::new(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match RerankConfigApi::get(&state).await {
                    Ok(cfg) => {
                        // Auto-select the current provider
                        let current = cfg.provider.as_str().to_string();
                        if selected_provider.get_untracked().is_none() {
                            selected_provider.set(Some(current));
                        }
                        config.set(Some(cfg));
                    }
                    Err(_) => {
                        config.set(None);
                    }
                }
                loading.set(false);
            });
        }
    });

    view! {
        <div class="flex h-full">
            // Left panel — provider list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
                <div class="px-6 py-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        "Reranking Providers"
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        "Configure cross-encoder reranking providers for retrieval precision"
                    </p>
                </div>

                <div class="flex-1 overflow-auto">
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">"Loading reranking providers..."</div>
                                </div>
                            }.into_any()
                        } else {
                            let current_provider = config.get()
                                .map(|c| c.provider.as_str().to_string())
                                .unwrap_or_else(|| "jina".to_string());
                            let is_enabled = config.get().map(|c| c.enabled).unwrap_or(false);

                            view! {
                                <div class="p-6 space-y-4">
                                    // Preset providers
                                    <div>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                            "Reranking Providers"
                                        </h2>
                                        <div class="grid grid-cols-1 gap-2">
                                            {RERANK_PRESETS.iter().map(|preset| {
                                                let key = preset.key.to_string();
                                                let name = preset.name.to_string();
                                                let icon_color = preset.icon_color.to_string();
                                                let default_model = preset.default_model.to_string();
                                                let first_char = name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                                let is_active_provider = current_provider == key;
                                                let key_click = key.clone();
                                                let key_check = key.clone();

                                                view! {
                                                    <button
                                                        on:click=move |_| {
                                                            selected_provider.set(Some(key_click.clone()));
                                                            set_show_add_form.set(false);
                                                        }
                                                        class=move || {
                                                            let base = "text-left p-3 rounded-lg border transition-all";
                                                            let is_sel = selected_provider.get().as_deref() == Some(&key_check) && !show_add_form.get();
                                                            if is_sel {
                                                                format!("{} bg-primary-subtle border-primary", base)
                                                            } else if is_active_provider {
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
                                                                        {name}
                                                                    </span>
                                                                    {if is_active_provider && is_enabled {
                                                                        view! {
                                                                            <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                                                "Active"
                                                                            </span>
                                                                        }.into_any()
                                                                    } else if is_active_provider {
                                                                        view! {
                                                                            <span class="px-1.5 py-0.5 bg-surface-sunken text-text-tertiary text-xs rounded shrink-0">
                                                                                "Selected"
                                                                            </span>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! { <span></span> }.into_any()
                                                                    }}
                                                                </div>
                                                                <div class="text-xs text-text-tertiary truncate">
                                                                    {default_model}
                                                                </div>
                                                            </div>
                                                        </div>
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>

                                    // Add Custom Provider button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                set_show_add_form.set(true);
                                                selected_provider.set(None);
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

            // Right panel — detail / add form
            <div class="flex-1 flex flex-col overflow-hidden">
                {move || {
                    if loading.get() {
                        view! { <div></div> }.into_any()
                    } else if show_add_form.get() {
                        view! {
                            <AddCustomProviderPanel
                                config=config
                                on_saved=move || {
                                    set_show_add_form.set(false);
                                    // Select the newly saved custom provider
                                    if let Some(cfg) = config.get() {
                                        selected_provider.set(Some(cfg.provider.as_str().to_string()));
                                    }
                                }
                                on_cancel=move || {
                                    set_show_add_form.set(false);
                                    // Re-select current provider
                                    if let Some(cfg) = config.get() {
                                        selected_provider.set(Some(cfg.provider.as_str().to_string()));
                                    }
                                }
                            />
                        }.into_any()
                    } else if let Some(sel_key) = selected_provider.get() {
                        if config.get().is_some() {
                            view! {
                                <ProviderDetailPanel
                                    provider_key=sel_key
                                    config=config
                                />
                            }.into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary">
                                <div class="text-center">
                                    <p class="text-lg">"Select a provider to configure"</p>
                                    <p class="text-sm text-text-tertiary mt-1">"or add a custom reranking provider"</p>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// Provider Detail Panel
// ============================================================================

#[component]
fn ProviderDetailPanel(
    provider_key: String,
    config: RwSignal<Option<RerankConfig>>,
) -> impl IntoView {
    let preset = RERANK_PRESETS.iter().find(|p| p.key == provider_key);
    let preset_name = preset.map(|p| p.name).unwrap_or("Custom").to_string();
    let preset_key = provider_key.clone();
    let default_api_base = preset.map(|p| p.default_api_base).unwrap_or("").to_string();

    // Initialize form state from config
    let rerank_cfg = config.get().unwrap_or_default();
    let is_current_provider = rerank_cfg.provider.as_str() == provider_key;

    let enabled = RwSignal::new(true);
    let api_base = RwSignal::new(if is_current_provider { rerank_cfg.api_base.clone() } else { String::new() });
    let api_key = RwSignal::new(if is_current_provider { rerank_cfg.api_key.clone() } else { String::new() });
    let selected_model = RwSignal::new({
        if is_current_provider && !rerank_cfg.model.is_empty() {
            Some(rerank_cfg.model.clone())
        } else {
            preset.map(|p| p.default_model.to_string())
        }
    });
    let models_list = RwSignal::new(Vec::<ModelOption>::new());
    let (probe_loading, set_probe_loading) = signal(false);
    let timeout_ms = RwSignal::new(if is_current_provider { rerank_cfg.timeout_ms } else { 5000 });
    let rerank_weight = RwSignal::new(if is_current_provider { rerank_cfg.rerank_weight } else { 0.6 });

    // Auto-probe to discover models on mount
    // Reranking providers (Jina, SiliconFlow, Voyage, vLLM) are OpenAI-compatible,
    // so we probe with protocol "openai" and the provider's base URL.
    {
        let probe_base = if is_current_provider && !rerank_cfg.api_base.is_empty() {
            rerank_cfg.api_base.clone()
        } else {
            default_api_base.clone()
        };
        let probe_key = if is_current_provider && !rerank_cfg.api_key.is_empty() {
            Some(rerank_cfg.api_key.clone())
        } else {
            None
        };
        set_probe_loading.set(true);
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            let base_url_opt = if probe_base.is_empty() { None } else { Some(probe_base) };
            match ProvidersApi::probe(
                &state,
                "openai",
                None,
                probe_key.as_deref(),
                base_url_opt.as_deref(),
            ).await {
                Ok(result) => {
                    let options: Vec<ModelOption> = result.models.into_iter().map(|m| {
                        ModelOption {
                            id: m.id.clone(),
                            name: m.name.clone(),
                            capabilities: m.capabilities.clone(),
                            source: result.model_source.clone(),
                        }
                    }).collect();
                    if !options.is_empty() {
                        models_list.set(options);
                    }
                }
                Err(_) => {
                    // Probe failed — models_list stays empty, user can enter custom model
                }
            }
            set_probe_loading.set(false);
        });
    }

    // Action states
    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (save_success, set_save_success) = signal(false);
    let (action_error, set_action_error) = signal(Option::<String>::None);

    // Build RerankConfig from form state
    let preset_key_for_build = preset_key.clone();
    let build_rerank_config = move || -> RerankConfig {
        RerankConfig {
            enabled: enabled.get(),
            provider: RerankProviderType::from_str_val(&preset_key_for_build),
            api_base: api_base.get(),
            api_key: api_key.get(),
            model: selected_model.get().unwrap_or_default(),
            timeout_ms: timeout_ms.get(),
            rerank_weight: rerank_weight.get(),
        }
    };

    // Test connection
    let build_for_test = build_rerank_config.clone();
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let rerank = build_for_test();
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match RerankConfigApi::test(&state, rerank).await {
                Ok(resp) => {
                    if resp.success {
                        set_test_result.set(Some((true, format!(
                            "Success! {} results, top score: {:.3}",
                            resp.results_count, resp.top_score
                        ))));
                    } else {
                        set_test_result.set(Some((false, format!(
                            "Failed: {}",
                            resp.error.unwrap_or_else(|| "Unknown error".to_string())
                        ))));
                    }
                }
                Err(e) => {
                    set_test_result.set(Some((false, format!("RPC error: {}", e))));
                }
            }
            set_testing.set(false);
        });
    };

    // Save handler
    let build_for_save = build_rerank_config.clone();
    let handle_save = move |_| {
        set_action_error.set(None);
        set_save_success.set(false);
        set_saving.set(true);

        let rerank = build_for_save();
        let rerank_clone = rerank.clone();
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match RerankConfigApi::update(&state, rerank_clone.clone()).await {
                Ok(_) => {
                    config.set(Some(rerank_clone));
                    set_save_success.set(true);
                    set_timeout(move || set_save_success.set(false), std::time::Duration::from_secs(2));
                }
                Err(e) => {
                    set_action_error.set(Some(format!("Save failed: {}", e)));
                }
            }
            set_saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {preset_name}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            "Cross-encoder reranking provider"
                        </p>
                    </div>
                    <div class="flex gap-1">
                        {if is_current_provider && rerank_cfg.enabled {
                            view! {
                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-primary-subtle text-primary">
                                    "Active"
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

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "API Key"
                    </label>
                    <SecretInput
                        value=Signal::derive(move || api_key.get())
                        on_change=move |v| api_key.set(v)
                        placeholder="Enter API key"
                        monospace=true
                    />
                </div>

                // Model
                {move || {
                    if probe_loading.get() {
                        view! {
                            <div class="text-xs text-text-tertiary animate-pulse">"Discovering models..."</div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
                <ModelSelector
                    models=Signal::derive(move || models_list.get())
                    selected=selected_model
                    allow_custom=true
                />

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Base URL"
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder=default_api_base.clone()
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
                        <span class="text-sm text-text-primary">"Enabled"</span>
                        <p class="text-xs text-text-tertiary">"Use this provider to rerank retrieval results"</p>
                    </div>
                </label>
            </div>

            // Parameters card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"PARAMETERS"</h3>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            "Timeout (ms)"
                        </label>
                        <input
                            type="number"
                            min="100"
                            value=move || timeout_ms.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                    timeout_ms.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            "Rerank Weight"
                        </label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            value=move || rerank_weight.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                    rerank_weight.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                        <p class="text-xs text-text-tertiary mt-1">"Blend: weight × rerank + (1-w) × original"</p>
                    </div>
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
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}

// ============================================================================
// Add Custom Provider Panel
// ============================================================================

#[component]
fn AddCustomProviderPanel(
    config: RwSignal<Option<RerankConfig>>,
    on_saved: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    // Form state
    let name = RwSignal::new(String::new());
    let api_base = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let selected_model = RwSignal::new(None::<String>);
    let models_list = RwSignal::new(Vec::<ModelOption>::new());
    let (probe_loading, set_probe_loading) = signal(false);
    let timeout_ms = RwSignal::new(5000u64);
    let rerank_weight = RwSignal::new(0.6f32);

    // Probe models when base_url is provided
    let trigger_custom_probe = move || {
        let base = api_base.get();
        if base.is_empty() {
            return;
        }
        let key = api_key.get();
        let key_opt = if key.is_empty() { None } else { Some(key) };
        set_probe_loading.set(true);
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match ProvidersApi::probe(
                &state,
                "openai",
                None,
                key_opt.as_deref(),
                Some(&base),
            ).await {
                Ok(result) => {
                    let options: Vec<ModelOption> = result.models.into_iter().map(|m| {
                        ModelOption {
                            id: m.id.clone(),
                            name: m.name.clone(),
                            capabilities: m.capabilities.clone(),
                            source: result.model_source.clone(),
                        }
                    }).collect();
                    if !options.is_empty() {
                        models_list.set(options);
                    }
                }
                Err(_) => {
                    // Probe failed — keep existing, user can enter custom model
                }
            }
            set_probe_loading.set(false);
        });
    };

    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (action_error, set_action_error) = signal(Option::<String>::None);

    // Build RerankConfig from form state
    let build_rerank_config = move || -> RerankConfig {
        RerankConfig {
            enabled: true,
            provider: RerankProviderType::Vllm, // Custom providers use vLLM-compatible API
            api_base: api_base.get(),
            api_key: api_key.get(),
            model: selected_model.get().unwrap_or_default(),
            timeout_ms: timeout_ms.get(),
            rerank_weight: rerank_weight.get(),
        }
    };

    // Test connection
    let build_for_test = build_rerank_config.clone();
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let rerank = build_for_test();
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match RerankConfigApi::test(&state, rerank).await {
                Ok(resp) => {
                    if resp.success {
                        set_test_result.set(Some((true, format!(
                            "Success! {} results, top score: {:.3}",
                            resp.results_count, resp.top_score
                        ))));
                    } else {
                        set_test_result.set(Some((false, format!(
                            "Failed: {}",
                            resp.error.unwrap_or_else(|| "Unknown error".to_string())
                        ))));
                    }
                }
                Err(e) => {
                    set_test_result.set(Some((false, format!("RPC error: {}", e))));
                }
            }
            set_testing.set(false);
        });
    };

    // Save handler — saves as vLLM provider with custom endpoint
    let build_for_save = build_rerank_config.clone();
    let handle_save = move |_| {
        // Validate
        if api_base.get().is_empty() {
            set_action_error.set(Some("API Base URL is required for custom providers".to_string()));
            return;
        }
        if selected_model.get().map_or(true, |m| m.is_empty()) {
            set_action_error.set(Some("Please select a model from the dropdown, or choose 'Enter custom model name' to type one manually".to_string()));
            return;
        }

        set_saving.set(true);
        set_action_error.set(None);

        let rerank = build_for_save();
        let rerank_clone = rerank.clone();
        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match RerankConfigApi::update(&state, rerank_clone.clone()).await {
                Ok(_) => {
                    config.set(Some(rerank_clone));
                    set_saving.set(false);
                    on_saved();
                }
                Err(e) => {
                    set_action_error.set(Some(format!("Save failed: {}", e)));
                    set_saving.set(false);
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
                            "Add Custom Provider"
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            "Configure a custom reranking endpoint (vLLM-compatible API)"
                        </p>
                    </div>
                    <button
                        on:click=move |_| on_cancel()
                        class="px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary border border-border rounded-lg hover:bg-surface-raised transition-colors"
                    >
                        "Cancel"
                    </button>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Provider name (optional display name)
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"PROVIDER INFO"</h3>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Name"
                    </label>
                    <input
                        type="text"
                        value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                        placeholder="e.g. My Local Reranker"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
            </div>

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"CONFIGURATION"</h3>

                // API Key
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "API Key"
                    </label>
                    <SecretInput
                        value=Signal::derive(move || api_key.get())
                        on_change=move |v| api_key.set(v)
                        placeholder="Enter API key (optional for local endpoints)"
                        monospace=true
                    />
                </div>

                // Model
                <ModelSelector
                    models=Signal::derive(move || models_list.get())
                    selected=selected_model
                    allow_custom=true
                />

                // API Base URL
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        "Base URL"
                        <span class="text-danger ml-1">"*"</span>
                    </label>
                    <input
                        type="text"
                        value=move || api_base.get()
                        on:input=move |ev| api_base.set(event_target_value(&ev))
                        placeholder="https://api.example.com/v1"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Discover models button
                <button
                    on:click=move |_| trigger_custom_probe()
                    disabled=move || probe_loading.get() || api_base.get().is_empty()
                    class="w-full px-4 py-2 border border-border rounded-lg text-sm text-text-secondary hover:text-primary hover:border-primary disabled:opacity-50 transition-colors"
                >
                    {move || if probe_loading.get() { "Discovering models..." } else { "Discover Models" }}
                </button>
            </div>

            // Parameters card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">"PARAMETERS"</h3>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            "Timeout (ms)"
                        </label>
                        <input
                            type="number"
                            min="100"
                            value=move || timeout_ms.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                    timeout_ms.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">
                            "Rerank Weight"
                        </label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            value=move || rerank_weight.get()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                    rerank_weight.set(v);
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                        <p class="text-xs text-text-tertiary mt-1">"Blend: weight × rerank + (1-w) × original"</p>
                    </div>
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

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
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
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { "Saving..." } else { "Add Provider" }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}
