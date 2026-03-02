//! AI Providers Configuration View
//!
//! Split-pane layout matching Embedding/Generation Providers:
//! - Left panel: Preset provider grid + configured provider list
//! - Right panel: Detail/form editor for selected provider
//! - Preset quick-setup for common AI services

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{ProvidersApi, ProviderInfo, ProviderConfig, TestResult};

// ============================================================================
// Preset Definitions
// ============================================================================

struct ProviderPreset {
    name: &'static str,
    protocol: &'static str,
    model: &'static str,
    base_url: &'static str,
    description: &'static str,
    api_key_placeholder: &'static str,
    icon_color: &'static str,
    needs_api_key: bool,
}

const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "anthropic",
        protocol: "anthropic",
        model: "claude-sonnet-4-5-20250514",
        base_url: "https://api.anthropic.com",
        description: "Claude by Anthropic",
        api_key_placeholder: "sk-ant-...",
        icon_color: "#D97757",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "openai",
        protocol: "openai",
        model: "gpt-4o",
        base_url: "https://api.openai.com/v1",
        description: "GPT models by OpenAI",
        api_key_placeholder: "sk-...",
        icon_color: "#10A37F",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "gemini",
        protocol: "gemini",
        model: "gemini-2.5-flash",
        base_url: "https://generativelanguage.googleapis.com",
        description: "Gemini by Google",
        api_key_placeholder: "AIza...",
        icon_color: "#4285F4",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "deepseek",
        protocol: "openai",
        model: "deepseek-chat",
        base_url: "https://api.deepseek.com/v1",
        description: "DeepSeek AI",
        api_key_placeholder: "sk-...",
        icon_color: "#4D6BFE",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "moonshot",
        protocol: "openai",
        model: "moonshot-v1-8k",
        base_url: "https://api.moonshot.cn/v1",
        description: "Kimi by Moonshot AI",
        api_key_placeholder: "sk-...",
        icon_color: "#5B21B6",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "volcengine",
        protocol: "openai",
        model: "doubao-1.5-pro-256k",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        description: "Doubao by Volcengine",
        api_key_placeholder: "sk-...",
        icon_color: "#FF6B35",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "siliconflow",
        protocol: "openai",
        model: "deepseek-ai/DeepSeek-V3",
        base_url: "https://api.siliconflow.cn/v1",
        description: "SiliconFlow AI Cloud",
        api_key_placeholder: "sk-...",
        icon_color: "#6C5CE7",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "zhipu",
        protocol: "openai",
        model: "GLM-5",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        description: "GLM by Zhipu AI",
        api_key_placeholder: "sk-...",
        icon_color: "#3B5998",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "minimax",
        protocol: "openai",
        model: "MiniMax-M2.5",
        base_url: "https://api.minimax.io/v1",
        description: "MiniMax AI",
        api_key_placeholder: "sk-...",
        icon_color: "#E84393",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "ollama",
        protocol: "ollama",
        model: "llama3.3",
        base_url: "http://localhost:11434",
        description: "Local models via Ollama",
        api_key_placeholder: "",
        icon_color: "#1D1D1F",
        needs_api_key: false,
    },
    ProviderPreset {
        name: "groq",
        protocol: "openai",
        model: "llama-3.3-70b-versatile",
        base_url: "https://api.groq.com/openai/v1",
        description: "Fast inference by Groq",
        api_key_placeholder: "gsk_...",
        icon_color: "#F55036",
        needs_api_key: true,
    },
    ProviderPreset {
        name: "openrouter",
        protocol: "openai",
        model: "anthropic/claude-sonnet-4-5",
        base_url: "https://openrouter.ai/api/v1",
        description: "Unified API gateway",
        api_key_placeholder: "sk-or-...",
        icon_color: "#6366F1",
        needs_api_key: true,
    },
];

fn find_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

// ============================================================================
// Main View
// ============================================================================

#[component]
pub fn ProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let providers = RwSignal::new(Vec::<ProviderInfo>::new());
    let selected = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load providers on mount
    spawn_local(async move {
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

    view! {
        <div class="flex h-full">
            // Left panel: Presets + Configured providers
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border">
                // Header
                <div class="px-6 py-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">"AI Providers"</h1>
                    <p class="mt-1 text-sm text-text-tertiary">
                        "Configure AI model providers. Select a preset or add a custom provider."
                    </p>
                </div>

                // Scrollable content
                <div class="flex-1 overflow-y-auto p-6 space-y-6">
                    {move || error.get().filter(|e| e.contains("Failed to load")).map(|_| view! {
                        <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <circle cx="12" cy="12" r="10"/>
                                <line x1="12" y1="16" x2="12" y2="12"/>
                                <line x1="12" y1="8" x2="12.01" y2="8"/>
                            </svg>
                            "Gateway not available — showing presets only"
                        </div>
                    })}

                    // Preset grid
                    <PresetGrid providers=providers selected=selected />

                    // Configured providers list
                    <ConfiguredProviders providers=providers selected=selected loading=loading />
                </div>
            </div>

            // Right panel: Detail/Editor
            <div class="w-7/12 min-w-0 overflow-y-auto">
                <ProviderDetailPanel
                    providers=providers
                    selected=selected
                    error=error
                />
            </div>
        </div>
    }
}

// ============================================================================
// Preset Grid
// ============================================================================

#[component]
fn PresetGrid(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Quick Setup"
            </h2>
            <div class="grid grid-cols-1 gap-2">
                {PRESETS.iter().map(|preset| {
                    let name = preset.name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;
                    let first_char = preset.name.chars().next().unwrap_or('?').to_uppercase().to_string();

                    let is_configured = move || {
                        providers.get().iter().any(|p| p.name == name)
                    };

                    let on_click = move |_| {
                        if is_configured() {
                            selected.set(Some(name.to_string()));
                        } else {
                            selected.set(Some(format!("__preset__{}", name)));
                        }
                    };

                    view! {
                        <button
                            on:click=on_click
                            class=move || {
                                let base = "text-left p-3 rounded-lg border transition-all";
                                let sel = selected.get();
                                let is_sel = sel.as_deref() == Some(name)
                                    || sel.as_deref() == Some(&format!("__preset__{}", name));
                                if is_sel {
                                    format!("{} bg-primary-subtle border-primary", base)
                                } else if is_configured() {
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
                                        <span class="font-medium text-text-primary text-sm capitalize truncate">
                                            {name}
                                        </span>
                                        {move || {
                                            let list = providers.get();
                                            let provider = list.iter().find(|p| p.name == name);
                                            if let Some(p) = provider {
                                                if p.is_default {
                                                    view! {
                                                        <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                            "Default"
                                                        </span>
                                                    }.into_any()
                                                } else if p.verified {
                                                    view! {
                                                        <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
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
                                    <div class="text-xs text-text-tertiary truncate">
                                        {description}
                                    </div>
                                </div>
                            </div>
                        </button>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

// ============================================================================
// Configured Providers List
// ============================================================================

#[component]
fn ConfiguredProviders(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    let on_add_custom = move |_| {
        selected.set(Some("__new__".to_string()));
    };

    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Configured Providers"
            </h2>

            {move || {
                if loading.get() {
                    view! {
                        <div class="text-center py-6 text-text-tertiary text-sm">"Loading..."</div>
                    }.into_any()
                } else {
                    let list = providers.get();
                    if list.is_empty() {
                        view! {
                            <div class="text-center py-6 text-text-tertiary text-sm border border-dashed border-border rounded-lg">
                                "No providers configured yet. Click a preset above to get started."
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-2">
                                {move || providers.get().into_iter().map(|provider| {
                                    let name = provider.name.clone();
                                    let name_sel = name.clone();
                                    let name_click = name.clone();
                                    let model = provider.model.clone();
                                    let verified = provider.verified;
                                    let is_default = provider.is_default;
                                    let protocol = provider.provider_type.clone().unwrap_or_default();

                                    let preset = find_preset(&name);
                                    let icon_color = preset.map(|p| p.icon_color).unwrap_or("#808080");
                                    let first_char = name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                    view! {
                                        <button
                                            on:click=move |_| selected.set(Some(name_click.clone()))
                                            class=move || {
                                                let base = "w-full text-left p-3 rounded-lg border transition-all flex items-center gap-3";
                                                if selected.get().as_deref() == Some(&name_sel) {
                                                    format!("{} bg-primary-subtle border-primary", base)
                                                } else {
                                                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                                }
                                            }
                                        >
                                            <div
                                                class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                style=format!("background-color: {}", icon_color)
                                            >
                                                {first_char.clone()}
                                            </div>
                                            <div class="flex-1 min-w-0">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-medium text-text-primary text-sm truncate">
                                                        {name.clone()}
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
                                                    {if !protocol.is_empty() {
                                                        view! {
                                                            <span class="px-1.5 py-0.5 bg-surface-sunken text-text-tertiary text-xs rounded shrink-0">
                                                                {protocol}
                                                            </span>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }}
                                                </div>
                                                <div class="text-xs text-text-tertiary truncate">{model}</div>
                                            </div>
                                            <div class=move || {
                                                if verified { "w-2 h-2 rounded-full bg-success shrink-0" }
                                                else { "w-2 h-2 rounded-full bg-text-tertiary shrink-0" }
                                            }></div>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }
            }}

            // Add Custom Provider button
            <div class="pt-2">
                <button
                    on:click=on_add_custom
                    class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                >
                    "+ Add Custom Provider"
                </button>
            </div>
        </div>
    }
}

// ============================================================================
// Detail Panel (Right Side)
// ============================================================================

#[component]
fn ProviderDetailPanel(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
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
    let form_timeout = RwSignal::new(300u64);
    let form_max_tokens = RwSignal::new(String::new());
    let form_temperature = RwSignal::new(String::new());

    let saving = RwSignal::new(false);
    let testing = RwSignal::new(false);
    let test_result = RwSignal::new(Option::<TestResult>::None);

    let is_new = move || {
        let sel = selected.get();
        sel.as_deref() == Some("__new__") || sel.as_ref().map(|s| s.starts_with("__preset__")).unwrap_or(false)
    };

    // Load form when selection changes
    Effect::new(move || {
        test_result.set(None);
        error.set(None);

        if let Some(sel) = selected.get() {
            if sel == "__new__" {
                form_name.set(String::new());
                form_protocol.set("openai".to_string());
                form_model.set(String::new());
                form_api_key.set(String::new());
                form_base_url.set(String::new());
                form_enabled.set(true);
                form_timeout.set(300);
                form_max_tokens.set(String::new());
                form_temperature.set(String::new());
            } else if let Some(preset_name) = sel.strip_prefix("__preset__") {
                if let Some(preset) = find_preset(preset_name) {
                    form_name.set(preset.name.to_string());
                    form_protocol.set(preset.protocol.to_string());
                    form_model.set(preset.model.to_string());
                    form_api_key.set(String::new());
                    form_base_url.set(preset.base_url.to_string());
                    form_enabled.set(true);
                    form_timeout.set(300);
                    form_max_tokens.set(String::new());
                    form_temperature.set(String::new());
                }
            } else {
                // Existing provider
                if let Some(provider) = providers.get().iter().find(|p| p.name == sel) {
                    form_name.set(provider.name.clone());
                    form_protocol.set(provider.provider_type.clone().unwrap_or_else(|| "openai".to_string()));
                    form_model.set(provider.model.clone());
                    form_api_key.set(String::new()); // Not returned for security
                    form_enabled.set(provider.enabled);
                    // Restore base_url from preset if available
                    if let Some(preset) = find_preset(&provider.name) {
                        form_base_url.set(preset.base_url.to_string());
                    } else {
                        form_base_url.set(String::new());
                    }
                }
            }
        }
    });

    // Build config from form
    let build_config = move || -> ProviderConfig {
        ProviderConfig {
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
            color: None,
            timeout_seconds: Some(form_timeout.get()),
            max_tokens: {
                let t = form_max_tokens.get();
                if t.is_empty() { None } else { t.parse().ok() }
            },
            temperature: {
                let t = form_temperature.get();
                if t.is_empty() { None } else { t.parse().ok() }
            },
            top_p: None,
            top_k: None,
        }
    };

    let on_save = move |_| {
        let name = form_name.get();
        if name.is_empty() {
            error.set(Some("Provider name is required".to_string()));
            return;
        }
        if form_model.get().is_empty() {
            error.set(Some("Model is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);
        let config = build_config();

        spawn_local(async move {
            let result = if is_new() {
                ProvidersApi::create(&state, name.clone(), config).await
            } else {
                ProvidersApi::update(&state, name.clone(), config).await
            };

            match result {
                Ok(()) => {
                    if let Ok(list) = ProvidersApi::list(&state).await {
                        providers.set(list);
                    }
                    selected.set(Some(name));
                }
                Err(e) => error.set(Some(format!("Failed to save: {}", e))),
            }
            saving.set(false);
        });
    };

    let on_test = move |_| {
        testing.set(true);
        test_result.set(None);
        let config = build_config();

        spawn_local(async move {
            match ProvidersApi::test_connection(&state, config).await {
                Ok(r) => test_result.set(Some(r)),
                Err(e) => error.set(Some(format!("Test failed: {}", e))),
            }
            testing.set(false);
        });
    };

    let on_set_default = move |_| {
        if let Some(name) = selected.get() {
            if name.starts_with("__") { return; }
            saving.set(true);
            spawn_local(async move {
                match ProvidersApi::set_default(&state, name).await {
                    Ok(()) => {
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                    }
                    Err(e) => error.set(Some(format!("Failed: {}", e))),
                }
                saving.set(false);
            });
        }
    };

    let on_delete = move |_| {
        if let Some(name) = selected.get() {
            if name.starts_with("__") { return; }
            saving.set(true);
            spawn_local(async move {
                match ProvidersApi::delete(&state, name).await {
                    Ok(()) => {
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                        selected.set(None);
                    }
                    Err(e) => error.set(Some(format!("Failed: {}", e))),
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex flex-col h-full">
            {move || {
                let sel = selected.get();
                if sel.is_none() {
                    return view! {
                        <div class="flex flex-col items-center justify-center flex-1 text-text-tertiary">
                            <svg class="w-12 h-12 mb-3 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"
                                    d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
                            </svg>
                            <span class="text-sm">"Select a provider to configure"</span>
                        </div>
                    }.into_any();
                }

                let sel = sel.unwrap();
                let preset_name = if sel.starts_with("__preset__") {
                    sel.strip_prefix("__preset__").map(|s| s.to_string())
                } else {
                    None
                };
                let title = if sel == "__new__" {
                    "Custom Provider".to_string()
                } else if let Some(ref pn) = preset_name {
                    format!("Setup {}", pn)
                } else {
                    sel.clone()
                };

                let preset_info = preset_name.as_deref()
                    .or(if !sel.starts_with("__") { Some(sel.as_str()) } else { None })
                    .and_then(find_preset);

                view! {
                    <div class="flex flex-col h-full">
                        // Header
                        <div class="px-6 py-4 border-b border-border">
                            <div class="flex items-center gap-3">
                                {if let Some(preset) = preset_info {
                                    let ch = preset.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                    view! {
                                        <div
                                            class="w-10 h-10 rounded-xl flex items-center justify-center text-white font-bold"
                                            style=format!("background-color: {}", preset.icon_color)
                                        >
                                            {ch}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="w-10 h-10 rounded-xl bg-surface-sunken flex items-center justify-center text-text-tertiary font-bold">
                                            "?"
                                        </div>
                                    }.into_any()
                                }}
                                <div class="flex-1">
                                    <h2 class="text-lg font-semibold text-text-primary capitalize">{title}</h2>
                                    {if let Some(preset) = preset_info {
                                        view! { <p class="text-xs text-text-tertiary">{preset.description}</p> }.into_any()
                                    } else {
                                        view! { <p class="text-xs text-text-tertiary">"Custom provider configuration"</p> }.into_any()
                                    }}
                                </div>
                            </div>
                        </div>

                        // Scrollable content
                        <div class="flex-1 overflow-y-auto p-6 space-y-6">
                            // Error
                            {move || error.get().filter(|e| !e.contains("Failed to load")).map(|e| view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                            })}

                            // Configuration form card
                            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Configuration"</h3>

                                // Name (editable only for new custom)
                                {move || if sel == "__new__" {
                                    view! {
                                        <div>
                                            <label class="block text-sm text-text-secondary mb-1">"Name"</label>
                                            <input
                                                type="text"
                                                prop:value=move || form_name.get()
                                                on:input=move |ev| form_name.set(event_target_value(&ev))
                                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                placeholder="my-provider"
                                            />
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }}

                                // Protocol
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Protocol"</label>
                                    <select
                                        prop:value=move || form_protocol.get()
                                        on:change=move |ev| form_protocol.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                    >
                                        <option value="openai">"OpenAI Compatible"</option>
                                        <option value="anthropic">"Anthropic"</option>
                                        <option value="gemini">"Google Gemini"</option>
                                        <option value="ollama">"Ollama"</option>
                                    </select>
                                </div>

                                // Model
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Model"</label>
                                    <input
                                        type="text"
                                        prop:value=move || form_model.get()
                                        on:input=move |ev| form_model.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        placeholder="model-name"
                                    />
                                </div>

                                // API Key
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"API Key"</label>
                                    <input
                                        type="password"
                                        prop:value=move || form_api_key.get()
                                        on:input=move |ev| form_api_key.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        placeholder=move || {
                                            preset_info.map(|p| p.api_key_placeholder).unwrap_or("sk-...")
                                        }
                                    />
                                    {move || if preset_info.map(|p| !p.needs_api_key).unwrap_or(false) {
                                        view! {
                                            <p class="mt-1 text-xs text-text-tertiary">"Not required for local providers"</p>
                                        }.into_any()
                                    } else if !is_new() {
                                        view! {
                                            <p class="mt-1 text-xs text-text-tertiary">"Leave empty to keep existing key"</p>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }}
                                </div>

                                // Base URL
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Base URL"</label>
                                    <input
                                        type="text"
                                        prop:value=move || form_base_url.get()
                                        on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        placeholder="https://api.example.com/v1"
                                    />
                                </div>

                                // Enabled
                                <label class="flex items-center gap-3 cursor-pointer">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || form_enabled.get()
                                        on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                                        class="w-4 h-4 rounded"
                                    />
                                    <div>
                                        <span class="text-sm text-text-primary">"Enabled"</span>
                                        <p class="text-xs text-text-tertiary">"Include this provider in the available providers list"</p>
                                    </div>
                                </label>
                            </div>

                            // Advanced Settings card
                            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Advanced Settings"</h3>
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Timeout (s)"</label>
                                    <input
                                        type="number"
                                        prop:value=move || form_timeout.get()
                                        on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { form_timeout.set(v); } }
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Max Tokens"</label>
                                    <input
                                        type="number"
                                        prop:value=move || form_max_tokens.get()
                                        on:input=move |ev| form_max_tokens.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        placeholder="Optional"
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm text-text-secondary mb-1">"Temperature"</label>
                                    <input
                                        type="number"
                                        step="0.1"
                                        prop:value=move || form_temperature.get()
                                        on:input=move |ev| form_temperature.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        placeholder="0.0 - 2.0"
                                    />
                                </div>
                            </div>

                            // Actions
                            <div class="space-y-2">
                                <div class="flex gap-2">
                                    <button
                                        on:click=on_save
                                        prop:disabled=move || saving.get()
                                        class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                    >
                                        {move || if saving.get() { "Saving..." } else { "Save" }}
                                    </button>
                                    <button
                                        on:click=on_test
                                        prop:disabled=move || testing.get() || saving.get()
                                        class="flex-1 px-4 py-2.5 bg-surface-raised border border-border hover:border-primary/40 text-text-secondary text-sm font-medium rounded-lg transition-colors disabled:opacity-50"
                                    >
                                        {move || if testing.get() { "Testing..." } else { "Test" }}
                                    </button>
                                </div>

                                {move || {
                                    let s = selected.get();
                                    let is_existing = s.as_ref().map(|s| !s.starts_with("__")).unwrap_or(false);
                                    if is_existing {
                                        view! {
                                            <div class="flex gap-2">
                                                <button
                                                    on:click=on_set_default
                                                    prop:disabled=move || saving.get()
                                                    class="flex-1 px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                                >
                                                    "Set Default"
                                                </button>
                                                <button
                                                    on:click=on_delete
                                                    prop:disabled=move || saving.get()
                                                    class="px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                >
                                                    "Delete"
                                                </button>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }
                                }}
                            </div>

                            // Test result
                            {move || test_result.get().map(|result| {
                                if result.success {
                                    view! {
                                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg">
                                            <div class="flex items-center gap-2 text-success text-sm">
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                                </svg>
                                                <span class="font-medium">"Connection successful"</span>
                                            </div>
                                            {result.latency_ms.map(|ms| view! {
                                                <p class="mt-1 text-xs text-success">{format!("Latency: {}ms", ms)}</p>
                                            })}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg">
                                            <div class="flex items-center gap-2 text-danger text-sm">
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                                </svg>
                                                <span class="font-medium">"Connection failed"</span>
                                            </div>
                                            {result.error.clone().map(|e| view! {
                                                <p class="mt-1 text-xs text-danger">{e}</p>
                                            })}
                                        </div>
                                    }.into_any()
                                }
                            })}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
