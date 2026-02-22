//! Shortcuts Settings View
//!
//! Provides UI for managing keyboard shortcuts configuration.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{ShortcutsConfig, ShortcutsConfigApi};

#[component]
pub fn ShortcutsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let config = RwSignal::new(ShortcutsConfig {
        summon: "Command+Grave".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Option+Space".to_string(),
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                match ShortcutsConfigApi::get(&state).await {
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
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Shortcuts Settings"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Configure keyboard shortcuts for quick access"
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">"Loading..."</div>
                        </div>
                    }.into_any()
                } else if let Some(err) = error.get() {
                    view! {
                        <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger">
                            {err}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-6">
                            <SummonSection config=config />
                            <CancelSection config=config />
                            <CommandPromptSection config=config />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SummonSection(config: RwSignal<ShortcutsConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let summon = RwSignal::new(config.get().summon.clone());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.summon = summon.get();

        spawn_local(async move {
            match ShortcutsConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    // Clear success message after 2 seconds
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
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Summon Hotkey"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Global hotkey to summon the Aleph window"
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Hotkey"
                    </label>
                    <input
                        type="text"
                        value=move || summon.get()
                        on:input=move |ev| summon.set(event_target_value(&ev))
                        placeholder="Command+Grave"
                        class="w-full px-3 py-2 border border-border rounded focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        "Format: Modifier+Key (e.g., Command+Grave, Option+Space)"
                    </p>
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
fn CancelSection(config: RwSignal<ShortcutsConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let cancel = RwSignal::new(config.get().cancel.clone().unwrap_or_default());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        let cancel_value = cancel.get();
        cfg.cancel = if cancel_value.is_empty() {
            None
        } else {
            Some(cancel_value)
        };

        spawn_local(async move {
            match ShortcutsConfigApi::update(&state, cfg).await {
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
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Cancel Hotkey"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Hotkey to cancel current operation (optional)"
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Hotkey"
                    </label>
                    <input
                        type="text"
                        value=move || cancel.get()
                        on:input=move |ev| cancel.set(event_target_value(&ev))
                        placeholder="Escape"
                        class="w-full px-3 py-2 border border-border rounded focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        "Leave empty to disable"
                    </p>
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
fn CommandPromptSection(config: RwSignal<ShortcutsConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let command_prompt = RwSignal::new(config.get().command_prompt.clone());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.command_prompt = command_prompt.get();

        spawn_local(async move {
            match ShortcutsConfigApi::update(&state, cfg).await {
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
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Command Prompt Hotkey"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Hotkey to open command completion prompt"
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Hotkey"
                    </label>
                    <input
                        type="text"
                        value=move || command_prompt.get()
                        on:input=move |ev| command_prompt.set(event_target_value(&ev))
                        placeholder="Option+Space"
                        class="w-full px-3 py-2 border border-border rounded focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        "Format: Modifier+Key (e.g., Option+Space, Control+K)"
                    </p>
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
