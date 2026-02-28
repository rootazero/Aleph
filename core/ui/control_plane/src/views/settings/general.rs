use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::ev::Event;
use crate::context::DashboardState;
use crate::api::{GeneralConfig, GeneralConfigApi};

#[component]
pub fn GeneralView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let (config, set_config) = signal(Some(GeneralConfig {
        default_provider: None,
        language: None,
    }));
    let (loading, set_loading) = signal(true);
    let (saving, set_saving) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // Store save_config in a StoredValue to avoid closure capture issues
    let save_config_fn = store_value(move || {
        if !state.is_connected.get() { return; }
        if let Some(cfg) = config.get() {
            set_saving.set(true);
            set_error.set(None);

            spawn_local(async move {
                match GeneralConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        set_saving.set(false);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to save config: {}", e)));
                        set_saving.set(false);
                    }
                }
            });
        }
    });

    // Load config on mount — use spawn_local directly for robustness
    // (Effect::new can have scope issues in reactive closures)
    spawn_local(async move {
        match GeneralConfigApi::get(&state).await {
            Ok(cfg) => {
                set_config.set(Some(cfg));
                set_loading.set(false);
            }
            Err(e) => {
                set_error.set(Some(format!("Failed to load config: {}", e)));
                set_loading.set(false);
            }
        }
    });

    view! {
        <div class="p-8 max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    "General Settings"
                </h1>
                <p class="text-text-secondary">
                    "Configure general application settings"
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">"Loading..."</div>
                        </div>
                    }.into_any()
                } else if let Some(cfg) = config.get() {
                    view! {
                        <div class="space-y-6">
                            {move || {
                                match error.get() {
                                    Some(e) if e.contains("Send failed") || e.contains("Failed to load") || e.contains("Failed to save config: Send") => {
                                        // Connection-related error: show as info banner
                                        Some(view! {
                                            <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2">
                                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                    <circle cx="12" cy="12" r="10"/>
                                                    <line x1="12" y1="16" x2="12" y2="12"/>
                                                    <line x1="12" y1="8" x2="12.01" y2="8"/>
                                                </svg>
                                                "Gateway not available — showing default settings"
                                            </div>
                                        }.into_any())
                                    }
                                    Some(e) => {
                                        // Real error
                                        Some(view! {
                                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                                {e}
                                            </div>
                                        }.into_any())
                                    }
                                    None => None,
                                }
                            }}

                            <LanguageSection
                                language=cfg.language.clone()
                                on_change=move |lang| {
                                    if let Some(mut c) = config.get() {
                                        c.language = lang;
                                        set_config.set(Some(c));
                                        save_config_fn.with_value(|f| f());
                                    }
                                }
                            />

                            {move || {
                                if saving.get() {
                                    Some(view! {
                                        <div class="p-3 bg-primary-subtle border border-primary/20 rounded-lg text-primary text-sm">
                                            "Saving..."
                                        </div>
                                    })
                                } else {
                                    None
                                }
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="text-text-secondary">"No configuration loaded"</div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn LanguageSection(
    language: Option<String>,
    on_change: impl Fn(Option<String>) + 'static + Copy,
) -> impl IntoView {
    let (selected, set_selected) = signal(language.unwrap_or_else(|| "system".to_string()));

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">"Language"</h2>

            <div>
                <label class="block text-sm font-medium text-text-secondary mb-2">
                    "Interface Language"
                </label>
                <select
                    prop:value=move || selected.get()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        set_selected.set(value.clone());
                        let lang = if value == "system" { None } else { Some(value) };
                        on_change(lang);
                    }
                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                >
                    <option value="system">"System Default"</option>
                    <option value="en">"English"</option>
                    <option value="zh-Hans">"简体中文"</option>
                    <option value="zh-Hant">"繁體中文"</option>
                    <option value="ja">"日本語"</option>
                    <option value="ko">"한국어"</option>
                </select>
            </div>
        </div>
    }
}

