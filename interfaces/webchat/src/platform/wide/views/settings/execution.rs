//! Execution engine settings page

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionConfig {
    pub default_timeout_secs: u64,
    pub max_iterations: usize,
}

struct ExecutionConfigApi;

impl ExecutionConfigApi {
    async fn get(state: &DashboardState) -> Result<ExecutionConfig, String> {
        let result = state.rpc_call("execution_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    async fn update(state: &DashboardState, config: &ExecutionConfig) -> Result<(), String> {
        let params = serde_json::to_value(config).map_err(|e| e.to_string())?;
        state.rpc_call("execution_config.update", params).await?;
        Ok(())
    }
}

fn format_duration(secs: u64) -> String {
    if secs >= 86400 {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        if hours > 0 {
            format!("{days} days {hours} hours")
        } else {
            format!("{days} days")
        }
    } else if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins > 0 {
            format!("{hours} hours {mins} min")
        } else {
            format!("{hours} hours")
        }
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} sec")
    }
}

#[component]
#[must_use]
pub fn ExecutionView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let config = RwSignal::new(ExecutionConfig::default());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load on mount
    {
        spawn_local(async move {
            match ExecutionConfigApi::get(&state).await {
                Ok(c) => {
                    config.set(c);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| e.to_string(),
                    )));
                    loading.set(false);
                }
            }
        });
    }

    let save = move |_| {
        let state = state;
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let c = config.get();
            match ExecutionConfigApi::update(&state, &c).await {
                Ok(()) => saving.set(false),
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| e.to_string(),
                    )));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto">
            <h1 class="text-2xl font-bold mb-6 text-text-primary">{t!(i18n, execution_settings.title)}</h1>

            <Show when=move || loading.get()>
                <p class="text-text-secondary">{t!(i18n, execution_settings.loading)}</p>
            </Show>

            <Show when=move || !loading.get()>
                <div class="space-y-6">
                    // Default Timeout
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, execution_settings.default_timeout_title)}</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            {t!(i18n, execution_settings.default_timeout_description)}
                        </p>
                        <div class="flex items-center gap-4">
                            <input
                                type="number"
                                min="60"
                                max="604800"
                                class="w-40 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                                prop:value=move || config.get().default_timeout_secs.to_string()
                                on:change=move |ev| {
                                    let val: u64 = event_target_value(&ev).parse().unwrap_or(172_800);
                                    config.update(|c| c.default_timeout_secs = val);
                                }
                            />
                            <span class="text-sm text-text-secondary">
                                {t!(i18n, execution_settings.seconds_prefix)}
                                {move || format_duration(config.get().default_timeout_secs)}
                                ")"
                            </span>
                        </div>
                    </div>

                    // Max Iterations
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">{t!(i18n, execution_settings.max_iterations_title)}</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            {t!(i18n, execution_settings.max_iterations_description)}
                        </p>
                        <input
                            type="number"
                            min="5"
                            max="10000"
                            class="w-40 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                            prop:value=move || config.get().max_iterations.to_string()
                            on:change=move |ev| {
                                let val: usize = event_target_value(&ev).parse().unwrap_or(200);
                                config.update(|c| c.max_iterations = val);
                            }
                        />
                    </div>

                    // Error display
                    <Show when=move || error.get().is_some()>
                        <div class="text-red-500 text-sm">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    // Save button
                    <button
                        class="px-4 py-2 bg-accent-primary text-white rounded-lg hover:bg-accent-primary/90 disabled:opacity-50"
                        disabled=move || saving.get()
                        on:click=save
                    >
                        {move || if saving.get() {
                            t_string!(i18n, execution_settings.saving).to_string()
                        } else {
                            t_string!(i18n, execution_settings.save).to_string()
                        }}
                    </button>
                </div>
            </Show>
        </div>
    }
}
