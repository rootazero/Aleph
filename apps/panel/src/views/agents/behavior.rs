// Behavior Tab — migrated from settings/agent.rs
// Reuses existing AgentConfigApi for file ops, code exec, and general settings.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::{AgentConfig, AgentConfigApi, FileOpsConfig, CodeExecConfig};
use crate::context::DashboardState;

#[component]
pub fn BehaviorTab() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(Option::<AgentConfig>::None);
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let error_message = RwSignal::new(Option::<String>::None);
    let success_message = RwSignal::new(Option::<String>::None);

    // Load configuration on mount
    spawn_local(async move {
        match AgentConfigApi::get(&state).await {
            Ok(cfg) => {
                config.set(Some(cfg));
                is_loading.set(false);
            }
            Err(e) => {
                error_message.set(Some(format!("Failed to load: {}", e)));
                is_loading.set(false);
            }
        }
    });

    view! {
        <div class="space-y-6">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">"Loading configuration..."</div>
                        </div>
                    }.into_any();
                }

                let Some(cfg) = config.get() else {
                    return view! {
                        <div class="text-text-secondary">"No configuration available"</div>
                    }.into_any();
                };

                view! {
                    <div class="space-y-6">
                        {move || error_message.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                        })}

                        {move || success_message.get().map(|msg| view! {
                            <div class="p-3 bg-success-subtle border border-success/30 rounded-lg text-success text-sm">{msg}</div>
                        })}

                        <FileOpsSection config=cfg.file_ops.clone() on_change=move |new_config| {
                            if let Some(mut c) = config.get() {
                                c.file_ops = new_config;
                                config.set(Some(c));
                            }
                        } />

                        <CodeExecSection config=cfg.code_exec.clone() on_change=move |new_config| {
                            if let Some(mut c) = config.get() {
                                c.code_exec = new_config;
                                config.set(Some(c));
                            }
                        } />

                        <GeneralSection
                            web_browsing=cfg.web_browsing
                            max_iterations=cfg.max_iterations
                            auto_execute_threshold=cfg.auto_execute_threshold
                            max_tasks_per_graph=cfg.max_tasks_per_graph
                            task_timeout_seconds=cfg.task_timeout_seconds
                            sandbox_enabled=cfg.sandbox_enabled
                            on_change=move |field: &str, value: String| {
                                if let Some(mut c) = config.get() {
                                    match field {
                                        "web_browsing" => c.web_browsing = value.parse().unwrap_or(false),
                                        "max_iterations" => c.max_iterations = value.parse().unwrap_or(10),
                                        "auto_execute_threshold" => c.auto_execute_threshold = value.parse().unwrap_or(0.95),
                                        "max_tasks_per_graph" => c.max_tasks_per_graph = value.parse().unwrap_or(20),
                                        "task_timeout_seconds" => c.task_timeout_seconds = value.parse().unwrap_or(300),
                                        "sandbox_enabled" => c.sandbox_enabled = value.parse().unwrap_or(true),
                                        _ => {}
                                    }
                                    config.set(Some(c));
                                }
                            }
                        />

                        <div class="flex justify-end pt-4 border-t border-border">
                            <button
                                on:click=move |_| {
                                    if let Some(cfg) = config.get() {
                                        is_saving.set(true);
                                        error_message.set(None);
                                        success_message.set(None);
                                        spawn_local(async move {
                                            match AgentConfigApi::update(&state, cfg).await {
                                                Ok(_) => {
                                                    success_message.set(Some("Configuration saved".to_string()));
                                                    is_saving.set(false);
                                                }
                                                Err(e) => {
                                                    error_message.set(Some(format!("Failed to save: {}", e)));
                                                    is_saving.set(false);
                                                }
                                            }
                                        });
                                    }
                                }
                                disabled=move || is_saving.get()
                                class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            >
                                {move || if is_saving.get() { "Saving..." } else { "Save Configuration" }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[component]
fn FileOpsSection(
    config: FileOpsConfig,
    on_change: impl Fn(FileOpsConfig) + 'static + Copy,
) -> impl IntoView {
    let enabled = RwSignal::new(config.enabled);
    let max_file_size = RwSignal::new(config.max_file_size);
    let require_write_confirm = RwSignal::new(config.require_confirmation_for_write);
    let require_delete_confirm = RwSignal::new(config.require_confirmation_for_delete);
    let allowed_paths = StoredValue::new(config.allowed_paths.clone());
    let denied_paths = StoredValue::new(config.denied_paths.clone());

    let update_config = move || {
        on_change(FileOpsConfig {
            enabled: enabled.get(),
            allowed_paths: allowed_paths.get_value(),
            denied_paths: denied_paths.get_value(),
            max_file_size: max_file_size.get(),
            require_confirmation_for_write: require_write_confirm.get(),
            require_confirmation_for_delete: require_delete_confirm.get(),
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">"File Operations"</h2>
            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Enable File Operations"</label>
                    <input type="checkbox" checked=move || enabled.get()
                        on:change=move |ev| { enabled.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Max File Size (bytes)"</label>
                    <input type="number" value=move || max_file_size.get()
                        on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { max_file_size.set(v); update_config(); } }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Confirm Writes"</label>
                    <input type="checkbox" checked=move || require_write_confirm.get()
                        on:change=move |ev| { require_write_confirm.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Confirm Deletes"</label>
                    <input type="checkbox" checked=move || require_delete_confirm.get()
                        on:change=move |ev| { require_delete_confirm.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
            </div>
        </div>
    }
}

#[component]
fn CodeExecSection(
    config: CodeExecConfig,
    on_change: impl Fn(CodeExecConfig) + 'static + Copy,
) -> impl IntoView {
    let enabled = RwSignal::new(config.enabled);
    let sandbox_enabled = RwSignal::new(config.sandbox_enabled);
    let allow_network = RwSignal::new(config.allow_network);
    let timeout_seconds = RwSignal::new(config.timeout_seconds);
    let default_runtime = RwSignal::new(config.default_runtime.clone());
    let allowed_runtimes = StoredValue::new(config.allowed_runtimes.clone());
    let working_directory = StoredValue::new(config.working_directory.clone());
    let pass_env = StoredValue::new(config.pass_env.clone());
    let blocked_commands = StoredValue::new(config.blocked_commands.clone());

    let update_config = move || {
        on_change(CodeExecConfig {
            enabled: enabled.get(),
            default_runtime: default_runtime.get(),
            timeout_seconds: timeout_seconds.get(),
            sandbox_enabled: sandbox_enabled.get(),
            allowed_runtimes: allowed_runtimes.get_value(),
            allow_network: allow_network.get(),
            working_directory: working_directory.get_value(),
            pass_env: pass_env.get_value(),
            blocked_commands: blocked_commands.get_value(),
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">"Code Execution"</h2>
            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Enable Code Execution"</label>
                    <input type="checkbox" checked=move || enabled.get()
                        on:change=move |ev| { enabled.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Sandbox Mode"</label>
                    <input type="checkbox" checked=move || sandbox_enabled.get()
                        on:change=move |ev| { sandbox_enabled.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Allow Network"</label>
                    <input type="checkbox" checked=move || allow_network.get()
                        on:change=move |ev| { allow_network.set(event_target_checked(&ev)); update_config(); }
                        class="w-4 h-4" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Timeout (seconds)"</label>
                    <input type="number" value=move || timeout_seconds.get()
                        on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { timeout_seconds.set(v); update_config(); } }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Default Runtime"</label>
                    <select prop:value=move || default_runtime.get()
                        on:change=move |ev| { default_runtime.set(event_target_value(&ev)); update_config(); }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary">
                        <option value="shell">"Shell"</option>
                        <option value="python">"Python"</option>
                        <option value="node">"Node.js"</option>
                    </select>
                </div>
            </div>
        </div>
    }
}

#[component]
fn GeneralSection(
    web_browsing: bool,
    max_iterations: usize,
    auto_execute_threshold: f32,
    max_tasks_per_graph: usize,
    task_timeout_seconds: u64,
    sandbox_enabled: bool,
    on_change: impl Fn(&str, String) + 'static + Copy,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">"General Settings"</h2>
            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Web Browsing"</label>
                    <input type="checkbox" checked=web_browsing
                        on:change=move |ev| on_change("web_browsing", event_target_checked(&ev).to_string())
                        class="w-4 h-4" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Max Iterations"</label>
                    <input type="number" value=max_iterations
                        on:input=move |ev| on_change("max_iterations", event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Auto Execute Threshold"</label>
                    <input type="number" step="0.01" min="0" max="1" value=auto_execute_threshold
                        on:input=move |ev| on_change("auto_execute_threshold", event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Max Tasks Per Graph"</label>
                    <input type="number" value=max_tasks_per_graph
                        on:input=move |ev| on_change("max_tasks_per_graph", event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Task Timeout (seconds)"</label>
                    <input type="number" value=task_timeout_seconds
                        on:input=move |ev| on_change("task_timeout_seconds", event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary" />
                </div>
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Sandbox Enabled"</label>
                    <input type="checkbox" checked=sandbox_enabled
                        on:change=move |ev| on_change("sandbox_enabled", event_target_checked(&ev).to_string())
                        class="w-4 h-4" />
                </div>
            </div>
        </div>
    }
}
