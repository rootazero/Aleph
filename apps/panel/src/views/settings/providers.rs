//! AI Providers Configuration View
//!
//! Split-pane layout matching Embedding/Generation Providers:
//! - Left panel: Preset provider grid + configured provider list
//! - Right panel: Detail/form editor for selected provider
//! - Preset quick-setup for common AI services

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{ProvidersApi, ProviderInfo, ProviderConfig, TestResult, OAuthStatus};
use crate::components::model_selector::{ModelSelector, ModelOption};
use crate::components::probe_indicator::ProbeStatus;
use crate::components::api_key_input::ApiKeyInput;
use crate::preset_data::{PRESETS, OAUTH_PRESETS, find_preset};

/// Map OAuth preset name to the canonical name used in config (e.g. "codex" → "chatgpt").
fn canonical_oauth_name(name: &str) -> &'static str {
    match name {
        "codex" => "chatgpt",
        other => {
            // Return a static str — for known presets only
            OAUTH_PRESETS.iter().find(|p| p.name == other).map(|p| p.name).unwrap_or("chatgpt")
        }
    }
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

                    // Subscription login section (OAuth providers)
                    <SubscriptionLoginSection providers=providers selected=selected />

                    // Preset grid (badges shown inline for configured providers)
                    <PresetGrid providers=providers selected=selected />

                    // Custom providers (not matching any preset)
                    <CustomProvidersList providers=providers selected=selected />

                    // Add Custom Provider button
                    <div class="pt-2">
                        <button
                            on:click=move |_| selected.set(Some("__new__".to_string()))
                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                        >
                            "+ Add Custom Provider"
                        </button>
                    </div>
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
// Subscription Login Section (OAuth providers)
// ============================================================================

#[component]
fn SubscriptionLoginSection(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    // Track OAuth connection status for each OAuth preset
    let oauth_statuses: Vec<(&'static str, RwSignal<Option<bool>>)> = OAUTH_PRESETS.iter()
        .map(|preset| (preset.name, RwSignal::new(None::<bool>)))
        .collect();
    let oauth_statuses = std::rc::Rc::new(oauth_statuses);

    // Query OAuth status on mount and when providers change
    {
        let oauth_statuses = oauth_statuses.clone();
        Effect::new(move || {
            let _ = providers.get(); // track providers changes
            let state = expect_context::<DashboardState>();
            for (name, status_signal) in oauth_statuses.iter() {
                let name = name.to_string();
                let status_signal = *status_signal;
                spawn_local(async move {
                    match ProvidersApi::oauth_status(&state, name).await {
                        Ok(status) => status_signal.set(Some(status.connected)),
                        Err(_) => status_signal.set(Some(false)),
                    }
                });
            }
        });
    }

    let oauth_statuses_view = oauth_statuses.clone();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Subscription Login"
            </h2>
            <div class="space-y-2">
                {OAUTH_PRESETS.iter().enumerate().map(|(idx, preset)| {
                    let name = preset.name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;
                    let first_char = preset.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                    let oauth_connected = oauth_statuses_view[idx].1;

                    // OAuth providers may be stored under canonical name (e.g. "chatgpt" for "codex")
                    let canonical = canonical_oauth_name(name);

                    let is_configured = move || {
                        providers.get().iter().any(|p| p.name == name || p.name == canonical)
                    };

                    let on_click = move |_| {
                        if is_configured() {
                            // Select by the name that actually exists in providers list
                            let actual_name = providers.get().iter()
                                .find(|p| p.name == name || p.name == canonical)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| name.to_string());
                            selected.set(Some(actual_name));
                        } else {
                            selected.set(Some(format!("__preset__{}", name)));
                        }
                    };

                    view! {
                        <button
                            on:click=on_click
                            class=move || {
                                let base = "w-full text-left p-4 rounded-xl border-2 transition-all";
                                let sel = selected.get();
                                let is_sel = sel.as_deref() == Some(name)
                                    || sel.as_deref() == Some(canonical)
                                    || sel.as_deref() == Some(&format!("__preset__{}", name));
                                let connected = oauth_connected.get().unwrap_or(false);
                                if is_sel {
                                    format!("{} bg-primary-subtle border-primary", base)
                                } else if connected {
                                    format!("{} bg-surface-raised border-success/30 hover:border-primary/40", base)
                                } else {
                                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                }
                            }
                        >
                            <div class="flex items-center gap-3">
                                <div
                                    class="w-10 h-10 rounded-xl flex items-center justify-center text-white text-sm font-bold shrink-0"
                                    style=format!("background-color: {}", icon_color)
                                >
                                    {first_char}
                                </div>
                                <div class="flex-1 min-w-0">
                                    <div class="flex items-center gap-2">
                                        <span class="font-semibold text-text-primary text-sm capitalize">
                                            {name}
                                        </span>
                                        {move || {
                                            let connected = oauth_connected.get().unwrap_or(false);
                                            let list = providers.get();
                                            let provider = list.iter().find(|p| p.name == name || p.name == canonical);
                                            let is_default = provider.map_or(false, |p| p.is_default);
                                            if is_default {
                                                view! {
                                                    <span class="px-1.5 py-0.5 bg-primary-subtle text-primary text-xs rounded shrink-0">
                                                        "Default"
                                                    </span>
                                                }.into_any()
                                            } else if connected {
                                                view! {
                                                    <span class="px-1.5 py-0.5 bg-success-subtle text-success text-xs rounded shrink-0">
                                                        "Connected"
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <span class="px-1.5 py-0.5 bg-surface-sunken text-text-tertiary text-xs rounded shrink-0">
                                                        "Not connected"
                                                    </span>
                                                }.into_any()
                                            }
                                        }}
                                    </div>
                                    <div class="text-xs text-text-tertiary">{description}</div>
                                </div>
                                // Arrow icon
                                <svg class="w-4 h-4 text-text-tertiary shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/>
                                </svg>
                            </div>
                        </button>
                    }
                }).collect_view()}
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
                                <div class="relative shrink-0">
                                    <div
                                        class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold"
                                        style=format!("background-color: {}", icon_color)
                                    >
                                        {first_char}
                                    </div>
                                    {move || {
                                        let list = providers.get();
                                        let provider = list.iter().find(|p| p.name == name);
                                        let is_verified = provider.map_or(false, |p| p.verified);
                                        if is_verified {
                                            view! {
                                                <span class="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-success border-2 border-surface-raised" />
                                            }.into_any()
                                        } else {
                                            view! { <span /> }.into_any()
                                        }
                                    }}
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
// Custom Providers List (non-preset providers)
// ============================================================================

#[component]
fn CustomProvidersList(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let mut preset_names: Vec<&str> = PRESETS.iter().chain(OAUTH_PRESETS.iter()).map(|p| p.name).collect();
    // Also exclude canonical OAuth names (e.g. "chatgpt" for "codex")
    for preset in OAUTH_PRESETS.iter() {
        let canonical = canonical_oauth_name(preset.name);
        if !preset_names.contains(&canonical) {
            preset_names.push(canonical);
        }
    }

    view! {
        {move || {
            let list = providers.get();
            let custom: Vec<_> = list.into_iter()
                .filter(|p| !preset_names.contains(&p.name.as_str()))
                .collect();
            if custom.is_empty() {
                view! { <div></div> }.into_any()
            } else {
                view! {
                    <div>
                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                            "Custom Providers"
                        </h2>
                        <div class="grid grid-cols-1 gap-2">
                            {custom.into_iter().map(|p| {
                                let name = p.name.clone();
                                let name_click = name.clone();
                                let name_check = name.clone();
                                let model = p.model.clone();
                                let color = p.color.clone();
                                let is_default = p.is_default;
                                let verified = p.verified;
                                let first_char = name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                view! {
                                    <button
                                        on:click=move |_| selected.set(Some(name_click.clone()))
                                        class=move || {
                                            let base = "text-left p-3 rounded-lg border transition-all";
                                            let is_sel = selected.get().as_deref() == Some(&name_check);
                                            if is_sel {
                                                format!("{} bg-primary-subtle border-primary", base)
                                            } else {
                                                format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                                            }
                                        }
                                    >
                                        <div class="flex items-center gap-3">
                                            <div class="relative shrink-0">
                                                <div
                                                    class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold"
                                                    style=format!("background-color: {}", color)
                                                >
                                                    {first_char}
                                                </div>
                                                <span class=if verified {
                                                    "absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-success border-2 border-surface-raised"
                                                } else {
                                                    "absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-text-tertiary/30 border-2 border-surface-raised"
                                                } />
                                            </div>
                                            <div class="min-w-0">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-medium text-text-primary text-sm truncate">
                                                        {name}
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
                                                    {model}
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
    let oauth_status = RwSignal::new(Option::<OAuthStatus>::None);
    let oauth_loading = RwSignal::new(false);

    // Model discovery signals
    let models_list = RwSignal::new(Vec::<ModelOption>::new());
    let probe_status = RwSignal::new(ProbeStatus::Idle);
    let is_refreshing = RwSignal::new(false);

    // Sync form_model <-> selected_model for ModelSelector.
    // Guard with get_untracked() to prevent infinite reactive loop.
    let selected_model = RwSignal::new(None::<String>);
    Effect::new(move || {
        let m = form_model.get();
        if !m.is_empty() && selected_model.get_untracked() != Some(m.clone()) {
            selected_model.set(Some(m));
        }
    });
    Effect::new(move || {
        if let Some(m) = selected_model.get() {
            if form_model.get_untracked() != m {
                form_model.set(m);
            }
        }
    });

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
                // Existing provider — populate form with actual values
                if let Some(provider) = providers.get().iter().find(|p| p.name == sel) {
                    form_name.set(provider.name.clone());
                    form_protocol.set(provider.provider_type.clone().unwrap_or_else(|| "openai".to_string()));
                    form_model.set(provider.model.clone());
                    form_api_key.set(provider.api_key.clone().unwrap_or_default());
                    form_enabled.set(provider.enabled);
                    form_base_url.set(provider.base_url.clone().unwrap_or_default());
                    form_timeout.set(provider.timeout_seconds);
                    form_max_tokens.set(provider.max_tokens.map(|v| v.to_string()).unwrap_or_default());
                    form_temperature.set(provider.temperature.map(|v| v.to_string()).unwrap_or_default());

                    // Auto-probe to discover models (API key resolved from vault on server)
                    let provider_name = provider.name.clone();
                    let protocol = provider.provider_type.clone().unwrap_or_else(|| "openai".to_string());
                    probe_status.set(ProbeStatus::Loading);
                    is_refreshing.set(true);
                    spawn_local(async move {
                        match ProvidersApi::probe(
                            &state,
                            &protocol,
                            Some(&provider_name),
                            None,
                            None,
                        ).await {
                            Ok(result) => {
                                if result.success {
                                    let latency = result.latency_ms.unwrap_or(0);
                                    probe_status.set(ProbeStatus::Success { latency_ms: latency });
                                    let options: Vec<ModelOption> = result.models.into_iter().map(|m| {
                                        ModelOption {
                                            id: m.id.clone(),
                                            name: m.name.clone(),
                                            capabilities: m.capabilities.clone(),
                                            source: result.model_source.clone(),
                                        }
                                    }).collect();
                                    models_list.set(options);
                                } else {
                                    probe_status.set(ProbeStatus::Error { message: result.error.unwrap_or_else(|| "Connection failed".to_string()) });
                                    models_list.set(Vec::new());
                                }
                            }
                            Err(e) => {
                                probe_status.set(ProbeStatus::Error { message: e });
                                models_list.set(Vec::new());
                            }
                        }
                        is_refreshing.set(false);
                    });
                }
            }
        }
    });

    // Check OAuth status when an OAuth provider is selected
    Effect::new(move || {
        let sel = selected.get();
        let provider_name = sel.as_deref()
            .and_then(|s| s.strip_prefix("__preset__").or(Some(s)))
            .and_then(|name| if name.starts_with("__") { None } else { Some(name.to_string()) });

        if let Some(name) = provider_name {
            if find_preset(&name).map(|p| p.auth_type == "oauth").unwrap_or(false) {
                oauth_loading.set(true);
                let state = expect_context::<DashboardState>();
                spawn_local(async move {
                    match ProvidersApi::oauth_status(&state, name).await {
                        Ok(status) => oauth_status.set(Some(status)),
                        Err(_) => oauth_status.set(Some(OAuthStatus {
                            connected: false,
                            expires_in_seconds: None,
                            error: None,
                        })),
                    }
                    oauth_loading.set(false);
                });
                return;
            }
        }
        oauth_status.set(None);
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

    // Trigger probe: discover models (uses vault key when api_key is empty)
    let trigger_probe = move |api_key: String| {
        let protocol = form_protocol.get();
        let name = form_name.get();
        let name_opt = if name.is_empty() { None } else { Some(name) };
        let base_url = form_base_url.get();
        let base_url_opt = if base_url.is_empty() { None } else { Some(base_url) };
        let api_key_opt = if api_key.is_empty() { None } else { Some(api_key) };

        probe_status.set(ProbeStatus::Loading);
        is_refreshing.set(true);

        spawn_local(async move {
            let state = expect_context::<DashboardState>();
            match ProvidersApi::probe(
                &state,
                &protocol,
                name_opt.as_deref(),
                api_key_opt.as_deref(),
                base_url_opt.as_deref(),
            ).await {
                Ok(result) => {
                    if result.success {
                        let latency = result.latency_ms.unwrap_or(0);
                        probe_status.set(ProbeStatus::Success { latency_ms: latency });
                        let options: Vec<ModelOption> = result.models.into_iter().map(|m| {
                            ModelOption {
                                id: m.id.clone(),
                                name: m.name.clone(),
                                capabilities: m.capabilities.clone(),
                                source: result.model_source.clone(),
                            }
                        }).collect();
                        models_list.set(options);
                    } else {
                        let msg = result.error.unwrap_or_else(|| "Connection failed".to_string());
                        probe_status.set(ProbeStatus::Error { message: msg });
                        models_list.set(Vec::new());
                    }
                }
                Err(e) => {
                    probe_status.set(ProbeStatus::Error { message: e });
                    models_list.set(Vec::new());
                }
            }
            is_refreshing.set(false);
        });
    };

    // Refresh callback for ModelSelector
    let trigger_probe_refresh = trigger_probe.clone();
    let on_refresh_models = Callback::new(move |_: ()| {
        let key = form_api_key.get();
        trigger_probe_refresh(key);
    });

    // API key change callback for ApiKeyInput
    let trigger_probe_key = trigger_probe.clone();
    let on_api_key_change = Callback::new(move |key: String| {
        trigger_probe_key(key);
    });

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

                            // OAuth login section (for subscription providers like Codex)
                            {if preset_info.map(|p| p.auth_type == "oauth").unwrap_or(false) {
                                view! {
                                    <div class="space-y-6">
                                        // Connection Status card (reactive)
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Connection Status"</h3>
                                            {move || {
                                                let status = oauth_status.get();
                                                let is_connected = status.as_ref().map(|s| s.connected).unwrap_or(false);
                                                let loading = oauth_loading.get();

                                                if loading {
                                                    view! {
                                                        <div class="flex items-center gap-3">
                                                            <div class="w-3 h-3 rounded-full bg-text-tertiary animate-pulse"></div>
                                                            <span class="text-sm text-text-tertiary">"Checking..."</span>
                                                        </div>
                                                    }.into_any()
                                                } else if is_connected {
                                                    let expires = status.as_ref()
                                                        .and_then(|s| s.expires_in_seconds)
                                                        .map(|secs| {
                                                            let hours = secs / 3600;
                                                            let mins = (secs % 3600) / 60;
                                                            if hours > 0 {
                                                                format!("Expires in {}h {}m", hours, mins)
                                                            } else {
                                                                format!("Expires in {}m", mins)
                                                            }
                                                        });
                                                    view! {
                                                        <div>
                                                            <div class="flex items-center gap-3">
                                                                <div class="w-3 h-3 rounded-full bg-success"></div>
                                                                <span class="text-sm text-success font-medium">"Connected"</span>
                                                            </div>
                                                            {expires.map(|e| view! {
                                                                <p class="mt-1 text-xs text-text-tertiary">{e}</p>
                                                            })}
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <div class="flex items-center gap-3">
                                                            <div class="w-3 h-3 rounded-full bg-text-tertiary"></div>
                                                            <span class="text-sm text-text-secondary">"Not connected"</span>
                                                        </div>
                                                    }.into_any()
                                                }
                                            }}
                                            <p class="text-xs text-text-tertiary">
                                                "Use your ChatGPT Plus or Pro subscription to access Codex models. No API key needed."
                                            </p>
                                            // Login / Logout button (reactive)
                                            {move || {
                                                let is_connected = oauth_status.get().as_ref().map(|s| s.connected).unwrap_or(false);
                                                if is_connected {
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = "codex".to_string();
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_logout(&state, provider_name).await {
                                                                        Ok(()) => {
                                                                            oauth_status.set(Some(OAuthStatus {
                                                                                connected: false,
                                                                                expires_in_seconds: None,
                                                                                error: None,
                                                                            }));
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(format!("Logout failed: {}", e)));
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                            class="w-full px-4 py-2.5 bg-surface-sunken border border-border text-text-secondary text-sm font-medium rounded-xl hover:bg-surface-raised transition-colors"
                                                        >
                                                            "Logout"
                                                        </button>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = "codex".to_string();
                                                                oauth_loading.set(true);
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_login(&state, provider_name).await {
                                                                        Ok(status) => {
                                                                            oauth_status.set(Some(status));
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(format!("OAuth login failed: {}", e)));
                                                                        }
                                                                    }
                                                                    oauth_loading.set(false);
                                                                });
                                                            }
                                                            prop:disabled=move || oauth_loading.get()
                                                            class="w-full px-4 py-3 bg-[#10A37F] hover:bg-[#0d8c6d] disabled:opacity-50 text-white text-sm font-semibold rounded-xl transition-colors flex items-center justify-center gap-2"
                                                        >
                                                            <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24">
                                                                <path d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364l2.0201-1.1638a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.4091-.6765zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0974-2.3616l2.603-1.5018 2.6029 1.5018v3.0036l-2.6029 1.5018-2.603-1.5018z"/>
                                                            </svg>
                                                            {move || if oauth_loading.get() { "Logging in..." } else { "Login with ChatGPT" }}
                                                        </button>
                                                    }.into_any()
                                                }
                                            }}
                                        </div>

                                        // Model configuration card (simplified for OAuth)
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">"Configuration"</h3>
                                            <div>
                                                <ModelSelector
                                                    models=Signal::derive(move || models_list.get())
                                                    selected=selected_model
                                                    show_refresh=true
                                                    on_refresh=on_refresh_models.clone()
                                                    refreshing=Signal::derive(move || is_refreshing.get())
                                                    allow_custom=true
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">"Timeout (s)"</label>
                                                <input
                                                    type="number"
                                                    prop:value=move || form_timeout.get()
                                                    on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { form_timeout.set(v); } }
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                />
                                            </div>
                                        </div>

                                        // Save + Set Default + Delete
                                        <div class="space-y-2">
                                            <button
                                                on:click=on_save
                                                prop:disabled=move || saving.get()
                                                class="w-full px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                            >
                                                {move || if saving.get() { "Saving..." } else { "Save" }}
                                            </button>

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
                                    </div>
                                }.into_any()
                            } else {
                                // Standard API key provider view
                                view! {
                                    <div class="space-y-6">
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
                                                    <option value="chatgpt">"ChatGPT (Codex)"</option>
                                                </select>
                                            </div>

                                            // API Key (with auto-probe)
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">"API Key"</label>
                                                <ApiKeyInput
                                                    value=form_api_key
                                                    placeholder=preset_info.map(|p| p.api_key_placeholder).unwrap_or("sk-...")
                                                    probe_status=Signal::derive(move || probe_status.get())
                                                    on_key_change=on_api_key_change.clone()
                                                />
                                                {move || if preset_info.map(|p| !p.needs_api_key).unwrap_or(false) {
                                                    view! {
                                                        <p class="mt-1 text-xs text-text-tertiary">"Not required for local providers"</p>
                                                    }.into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }}
                                            </div>

                                            // Model (grouped dropdown with refresh)
                                            <div>
                                                <ModelSelector
                                                    models=Signal::derive(move || models_list.get())
                                                    selected=selected_model
                                                    show_refresh=true
                                                    on_refresh=on_refresh_models.clone()
                                                    refreshing=Signal::derive(move || is_refreshing.get())
                                                    allow_custom=true
                                                />
                                            </div>

                                            // Base URL
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">"Base URL"</label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_base_url.get()
                                                    on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder=move || {
                                                        preset_info.map(|p| format!("Default: {}", p.base_url)).unwrap_or_else(|| "https://api.example.com/v1".to_string())
                                                    }
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
                                                    on:click=on_test
                                                    prop:disabled=move || testing.get() || saving.get()
                                                    class="flex-1 px-4 py-2.5 bg-info text-white text-sm font-medium rounded-lg hover:bg-primary-hover transition-colors disabled:opacity-50"
                                                >
                                                    {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                                                </button>
                                                <button
                                                    on:click=on_save
                                                    prop:disabled=move || saving.get()
                                                    class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                                >
                                                    {move || if saving.get() { "Saving..." } else { "Save" }}
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
                                }.into_any()
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
