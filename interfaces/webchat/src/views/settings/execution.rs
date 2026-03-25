//! Execution engine settings page

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

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
            format!("{} days {} hours", days, hours)
        } else {
            format!("{} days", days)
        }
    } else if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins > 0 {
            format!("{} hours {} min", hours, mins)
        } else {
            format!("{} hours", hours)
        }
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{} sec", secs)
    }
}

#[component]
pub fn ExecutionView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let config = RwSignal::new(ExecutionConfig::default());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load on mount
    {
        let state = state.clone();
        spawn_local(async move {
            match ExecutionConfigApi::get(&state).await {
                Ok(c) => {
                    config.set(c);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    }

    let save = move |_| {
        let state = state.clone();
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let c = config.get();
            match ExecutionConfigApi::update(&state, &c).await {
                Ok(()) => saving.set(false),
                Err(e) => {
                    error.set(Some(e));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="p-8 max-w-5xl mx-auto">
            <h1 class="text-2xl font-bold mb-6 text-text-primary">"Execution"</h1>

            <Show when=move || loading.get()>
                <p class="text-text-secondary">"Loading..."</p>
            </Show>

            <Show when=move || !loading.get()>
                <div class="space-y-6">
                    // Default Timeout
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">"Default Agent Timeout"</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            "Maximum time an agent run can execute before being terminated. "
                            "Individual agents can override this value."
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
                                "seconds ("
                                {move || format_duration(config.get().default_timeout_secs)}
                                ")"
                            </span>
                        </div>
                    </div>

                    // Max Iterations
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">"Max Iterations"</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            "Maximum number of think-act loop iterations per agent run."
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
                        {move || if saving.get() { "Saving..." } else { "Save" }}
                    </button>
                </div>
            </Show>
        </div>
    }
}
