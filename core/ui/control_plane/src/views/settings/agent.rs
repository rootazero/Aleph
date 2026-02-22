use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::{AgentConfig, AgentConfigApi, FileOpsConfig, CodeExecConfig};
use crate::context::DashboardState;

#[component]
pub fn AgentView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let (config, set_config) = signal(Option::<AgentConfig>::None);
    let (is_loading, set_is_loading) = signal(true);
    let (is_saving, set_is_saving) = signal(false);
    let (error_message, set_error_message) = signal(Option::<String>::None);
    let (success_message, set_success_message) = signal(Option::<String>::None);

    // Load configuration
    Effect::new(move |_| {
        if state.is_connected.get() {
            spawn_local(async move {
                set_is_loading.set(true);
                set_error_message.set(None);

                match AgentConfigApi::get(&state).await {
                    Ok(cfg) => {
                        set_config.set(Some(cfg));
                        set_is_loading.set(false);
                    }
                    Err(e) => {
                        set_error_message.set(Some(format!("Failed to load configuration: {}", e)));
                        set_is_loading.set(false);
                    }
                }
            });
        }
    });

    // Save handler
    let handle_save = move |_| {
        if let Some(cfg) = config.get() {
            set_is_saving.set(true);
            set_error_message.set(None);
            set_success_message.set(None);

            spawn_local(async move {
                match AgentConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        set_success_message.set(Some("Configuration saved successfully".to_string()));
                        set_is_saving.set(false);
                    }
                    Err(e) => {
                        set_error_message.set(Some(format!("Failed to save: {}", e)));
                        set_is_saving.set(false);
                    }
                }
            });
        }
    };

    view! {
        <div class="p-6 max-w-6xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    "Agent Settings"
                </h1>
                <p class="text-text-secondary">
                    "Configure agent behavior, file operations, and code execution permissions"
                </p>
            </div>

            {move || {
                if is_loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">"Loading configuration..."</div>
                        </div>
                    }.into_any()
                } else if let Some(cfg) = config.get() {
                    view! {
                        <div class="space-y-6">
                            // Error/Success messages
                            {move || error_message.get().map(|msg| view! {
                                <div class="p-4 bg-danger-subtle border border-danger/30 rounded-lg text-danger">
                                    {msg}
                                </div>
                            })}

                            {move || success_message.get().map(|msg| view! {
                                <div class="p-4 bg-success-subtle border border-success/30 rounded-lg text-success">
                                    {msg}
                                </div>
                            })}

                            // File Operations Section
                            <FileOpsSection config=cfg.file_ops.clone() on_change=move |new_config| {
                                if let Some(mut cfg) = config.get() {
                                    cfg.file_ops = new_config;
                                    set_config.set(Some(cfg));
                                }
                            } />

                            // Code Execution Section
                            <CodeExecSection config=cfg.code_exec.clone() on_change=move |new_config| {
                                if let Some(mut cfg) = config.get() {
                                    cfg.code_exec = new_config;
                                    set_config.set(Some(cfg));
                                }
                            } />

                            // General Settings Section
                            <GeneralSettingsSection
                                web_browsing=cfg.web_browsing
                                max_iterations=cfg.max_iterations
                                auto_execute_threshold=cfg.auto_execute_threshold
                                max_tasks_per_graph=cfg.max_tasks_per_graph
                                task_timeout_seconds=cfg.task_timeout_seconds
                                sandbox_enabled=cfg.sandbox_enabled
                                on_change=move |field, value| {
                                    if let Some(mut cfg) = config.get() {
                                        match field {
                                            "web_browsing" => cfg.web_browsing = value.parse().unwrap_or(false),
                                            "max_iterations" => cfg.max_iterations = value.parse().unwrap_or(10),
                                            "auto_execute_threshold" => cfg.auto_execute_threshold = value.parse().unwrap_or(0.95),
                                            "max_tasks_per_graph" => cfg.max_tasks_per_graph = value.parse().unwrap_or(20),
                                            "task_timeout_seconds" => cfg.task_timeout_seconds = value.parse().unwrap_or(300),
                                            "sandbox_enabled" => cfg.sandbox_enabled = value.parse().unwrap_or(true),
                                            _ => {}
                                        }
                                        set_config.set(Some(cfg));
                                    }
                                }
                            />

                            // Save Button
                            <div class="flex justify-end pt-4 border-t border-border">
                                <button
                                    on:click=handle_save
                                    disabled=move || is_saving.get()
                                    class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                                >
                                    {move || if is_saving.get() { "Saving..." } else { "Save Configuration" }}
                                </button>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="text-text-secondary">"No configuration available"</div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// File Operations Section Component
#[component]
fn FileOpsSection(
    config: FileOpsConfig,
    on_change: impl Fn(FileOpsConfig) + 'static + Copy,
) -> impl IntoView {
    let (enabled, set_enabled) = signal(config.enabled);
    let (allowed_paths, set_allowed_paths) = signal(config.allowed_paths.clone());
    let (denied_paths, set_denied_paths) = signal(config.denied_paths.clone());
    let (max_file_size, set_max_file_size) = signal(config.max_file_size);
    let (require_write_confirm, set_require_write_confirm) = signal(config.require_confirmation_for_write);
    let (require_delete_confirm, set_require_delete_confirm) = signal(config.require_confirmation_for_delete);

    // Update parent when any field changes
    let update_config = move || {
        let new_config = FileOpsConfig {
            enabled: enabled.get(),
            allowed_paths: allowed_paths.get(),
            denied_paths: denied_paths.get(),
            max_file_size: max_file_size.get(),
            require_confirmation_for_write: require_write_confirm.get(),
            require_confirmation_for_delete: require_delete_confirm.get(),
        };
        on_change(new_config);
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">"File Operations"</h2>

            <div class="space-y-4">
                // Enable toggle
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Enable File Operations"</label>
                    <input
                        type="checkbox"
                        checked=move || enabled.get()
                        on:change=move |ev| {
                            set_enabled.set(event_target_checked(&ev));
                            update_config();
                        }
                        class="w-4 h-4"
                    />
                </div>

                // Max file size
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Maximum File Size (bytes)"
                    </label>
                    <input
                        type="number"
                        value=move || max_file_size.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse() {
                                set_max_file_size.set(val);
                                update_config();
                            }
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        {move || format!("≈ {} MB", max_file_size.get() / 1024 / 1024)}
                    </p>
                </div>

                // Confirmation toggles
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Require Confirmation for Write"</label>
                    <input
                        type="checkbox"
                        checked=move || require_write_confirm.get()
                        on:change=move |ev| {
                            set_require_write_confirm.set(event_target_checked(&ev));
                            update_config();
                        }
                        class="w-4 h-4"
                    />
                </div>

                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Require Confirmation for Delete"</label>
                    <input
                        type="checkbox"
                        checked=move || require_delete_confirm.get()
                        on:change=move |ev| {
                            set_require_delete_confirm.set(event_target_checked(&ev));
                            update_config();
                        }
                        class="w-4 h-4"
                    />
                </div>

                // Path lists (simplified for now)
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Allowed Paths"
                    </label>
                    <p class="text-xs text-text-tertiary mb-2">
                        "Comma-separated list of allowed paths (empty = all paths allowed)"
                    </p>
                    <input
                        type="text"
                        value=move || allowed_paths.get().join(", ")
                        on:input=move |ev| {
                            let paths: Vec<String> = event_target_value(&ev)
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            set_allowed_paths.set(paths);
                            update_config();
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono text-sm"
                        placeholder="~/Documents, ~/Downloads"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Denied Paths"
                    </label>
                    <p class="text-xs text-text-tertiary mb-2">
                        "Comma-separated list of denied paths (takes precedence over allowed)"
                    </p>
                    <input
                        type="text"
                        value=move || denied_paths.get().join(", ")
                        on:input=move |ev| {
                            let paths: Vec<String> = event_target_value(&ev)
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            set_denied_paths.set(paths);
                            update_config();
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono text-sm"
                        placeholder="~/.ssh, ~/.gnupg"
                    />
                </div>
            </div>
        </div>
    }
}

// Code Execution Section Component
#[component]
fn CodeExecSection(
    config: CodeExecConfig,
    on_change: impl Fn(CodeExecConfig) + 'static + Copy,
) -> impl IntoView {
    use std::rc::Rc;

    let (enabled, set_enabled) = signal(config.enabled);
    let (sandbox_enabled, set_sandbox_enabled) = signal(config.sandbox_enabled);
    let (allow_network, set_allow_network) = signal(config.allow_network);
    let (timeout_seconds, set_timeout_seconds) = signal(config.timeout_seconds);
    let (default_runtime, set_default_runtime) = signal(config.default_runtime.clone());

    let update_config = Rc::new(move || {
        let new_config = CodeExecConfig {
            enabled: enabled.get(),
            default_runtime: default_runtime.get(),
            timeout_seconds: timeout_seconds.get(),
            sandbox_enabled: sandbox_enabled.get(),
            allowed_runtimes: config.allowed_runtimes.clone(),
            allow_network: allow_network.get(),
            working_directory: config.working_directory.clone(),
            pass_env: config.pass_env.clone(),
            blocked_commands: config.blocked_commands.clone(),
        };
        on_change(new_config);
    });

    let update_config_1 = update_config.clone();
    let update_config_2 = update_config.clone();
    let update_config_3 = update_config.clone();
    let update_config_4 = update_config.clone();
    let update_config_5 = update_config.clone();

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">"Code Execution"</h2>

            <div class="space-y-4">
                <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-sm">
                    "Warning: Enabling code execution allows the agent to run arbitrary code. Use with caution."
                </div>

                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Enable Code Execution"</label>
                    <input
                        type="checkbox"
                        checked=move || enabled.get()
                        on:change=move |ev| {
                            set_enabled.set(event_target_checked(&ev));
                            update_config_1();
                        }
                        class="w-4 h-4"
                    />
                </div>

                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Sandbox Mode"</label>
                    <input
                        type="checkbox"
                        checked=move || sandbox_enabled.get()
                        on:change=move |ev| {
                            set_sandbox_enabled.set(event_target_checked(&ev));
                            update_config_2();
                        }
                        class="w-4 h-4"
                    />
                </div>

                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Allow Network Access"</label>
                    <input
                        type="checkbox"
                        checked=move || allow_network.get()
                        on:change=move |ev| {
                            set_allow_network.set(event_target_checked(&ev));
                            update_config_3();
                        }
                        class="w-4 h-4"
                        disabled=move || !sandbox_enabled.get()
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Execution Timeout (seconds)"
                    </label>
                    <input
                        type="number"
                        value=move || timeout_seconds.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse() {
                                set_timeout_seconds.set(val);
                                update_config_4();
                            }
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Default Runtime"
                    </label>
                    <select
                        prop:value=move || default_runtime.get()
                        on:change=move |ev| {
                            set_default_runtime.set(event_target_value(&ev));
                            update_config_5();
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    >
                        <option value="shell">"Shell"</option>
                        <option value="python">"Python"</option>
                        <option value="node">"Node.js"</option>
                    </select>
                </div>
            </div>
        </div>
    }
}

// General Settings Section Component
#[component]
fn GeneralSettingsSection(
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
            <h2 class="text-xl font-semibold text-text-primary mb-4">"General Settings"</h2>

            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Web Browsing"</label>
                    <input
                        type="checkbox"
                        checked=web_browsing
                        on:change=move |ev| {
                            on_change("web_browsing", event_target_checked(&ev).to_string());
                        }
                        class="w-4 h-4"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Max Iterations"
                    </label>
                    <input
                        type="number"
                        value=max_iterations
                        on:input=move |ev| {
                            on_change("max_iterations", event_target_value(&ev));
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Auto Execute Threshold"
                    </label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        value=auto_execute_threshold
                        on:input=move |ev| {
                            on_change("auto_execute_threshold", event_target_value(&ev));
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        "Confidence threshold for auto-execution (0.0 - 1.0)"
                    </p>
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Max Tasks Per Graph"
                    </label>
                    <input
                        type="number"
                        value=max_tasks_per_graph
                        on:input=move |ev| {
                            on_change("max_tasks_per_graph", event_target_value(&ev));
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "Task Timeout (seconds)"
                    </label>
                    <input
                        type="number"
                        value=task_timeout_seconds
                        on:input=move |ev| {
                            on_change("task_timeout_seconds", event_target_value(&ev));
                        }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                <div class="flex items-center justify-between">
                    <label class="text-sm font-medium text-text-secondary">"Sandbox Enabled"</label>
                    <input
                        type="checkbox"
                        checked=sandbox_enabled
                        on:change=move |ev| {
                            on_change("sandbox_enabled", event_target_checked(&ev).to_string());
                        }
                        class="w-4 h-4"
                    />
                </div>
            </div>
        </div>
    }
}
