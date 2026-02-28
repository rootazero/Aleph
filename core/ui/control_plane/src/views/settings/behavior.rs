//! Behavior Settings View
//!
//! Provides UI for managing behavior configuration (output mode, typing speed).

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{BehaviorConfig, BehaviorConfigApi};

#[component]
pub fn BehaviorView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let config = RwSignal::new(BehaviorConfig {
        output_mode: "typewriter".to_string(),
        typing_speed: 50,
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load config on mount
    spawn_local(async move {
        match BehaviorConfigApi::get(&state).await {
            Ok(cfg) => {
                config.set(cfg);
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load config: {}", e)));
                loading.set(false);
            }
        }
    });

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Behavior Settings"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Configure output mode and typing speed"
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">"Loading..."</div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-6">
                            {move || {
                                match error.get() {
                                    Some(e) if e.contains("Send failed") || e.contains("Failed to load") => {
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
                                        Some(view! {
                                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                                {e}
                                            </div>
                                        }.into_any())
                                    }
                                    None => None,
                                }
                            }}
                            <OutputModeSection config=config />
                            <TypingSpeedSection config=config />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn OutputModeSection(config: RwSignal<BehaviorConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let output_mode = RwSignal::new(config.get().output_mode.clone());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.output_mode = output_mode.get();

        spawn_local(async move {
            match BehaviorConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || {
                            save_success.set(false);
                        },
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Output Mode"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Choose how AI responses are displayed"
            </p>

            <div class="space-y-4">
                <div class="space-y-2">
                    <label class="flex items-center space-x-3 cursor-pointer">
                        <input
                            type="radio"
                            name="output_mode"
                            value="typewriter"
                            checked=move || output_mode.get() == "typewriter"
                            on:change=move |_| output_mode.set("typewriter".to_string())
                            class="w-4 h-4 text-primary focus:ring-primary/30"
                        />
                        <div>
                            <div class="font-medium text-text-primary">"Typewriter"</div>
                            <div class="text-sm text-text-tertiary">"Display responses character by character"</div>
                        </div>
                    </label>

                    <label class="flex items-center space-x-3 cursor-pointer">
                        <input
                            type="radio"
                            name="output_mode"
                            value="instant"
                            checked=move || output_mode.get() == "instant"
                            on:change=move |_| output_mode.set("instant".to_string())
                            class="w-4 h-4 text-primary focus:ring-primary/30"
                        />
                        <div>
                            <div class="font-medium text-text-primary">"Instant"</div>
                            <div class="text-sm text-text-tertiary">"Display complete responses immediately"</div>
                        </div>
                    </label>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                "Saved successfully"
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn TypingSpeedSection(config: RwSignal<BehaviorConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let typing_speed = RwSignal::new(config.get().typing_speed);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.typing_speed = typing_speed.get();

        spawn_local(async move {
            match BehaviorConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || {
                            save_success.set(false);
                        },
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Typing Speed"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Characters per second in typewriter mode (50-400)"
            </p>

            <div class="space-y-4">
                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-text-secondary">
                            "Speed: " {move || typing_speed.get()} " chars/sec"
                        </label>
                    </div>
                    <input
                        type="range"
                        min="50"
                        max="400"
                        step="10"
                        value=move || typing_speed.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                typing_speed.set(val);
                            }
                        }
                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                    <div class="flex justify-between text-xs text-text-tertiary mt-1">
                        <span>"Slow (50)"</span>
                        <span>"Fast (400)"</span>
                    </div>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                "Saved successfully"
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>
        </div>
    }
}
