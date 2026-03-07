//! MCP Configuration View
//!
//! Provides UI for managing MCP server configurations:
//! - List all MCP servers as cards
//! - Add/Edit/Delete servers via dialog
//! - Configure command, args, and environment variables

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::{McpConfigApi, McpServerConfig, McpServerInfo};
use crate::context::DashboardState;

/// Load MCP servers list from Gateway
fn load_servers(
    state: DashboardState,
    servers: RwSignal<Vec<McpServerInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match McpConfigApi::list(&state).await {
            Ok(list) => {
                servers.set(list);
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load MCP servers: {}", e)));
                loading.set(false);
            }
        }
    });
}

#[component]
pub fn McpView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let servers = RwSignal::new(Vec::<McpServerInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_dialog = RwSignal::new(false);
    let editing_server = RwSignal::new(Option::<String>::None);

    // Load servers when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_servers(state, servers, loading, error);
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary mb-1">
                            "MCP Servers"
                        </h1>
                        <p class="text-sm text-text-secondary">
                            "Manage Model Context Protocol server connections"
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| {
                                load_servers(state, servers, loading, error);
                            }
                        >
                            "Refresh"
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| {
                                editing_server.set(None);
                                show_dialog.set(true);
                            }
                        >
                            "+ Add Server"
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Servers List Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">
                        {move || format!("Configured Servers ({})", servers.get().len())}
                    </h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                                </div>
                            }.into_any()
                        } else if servers.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-border rounded">
                                    <div class="text-4xl mb-4">"🔧"</div>
                                    <p class="text-text-secondary">"No MCP servers configured"</p>
                                    <p class="text-xs text-text-tertiary mt-1">
                                        "Add MCP servers to extend AI capabilities with external tools"
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || servers.get()
                                        key=|server| server.name.clone()
                                        children=move |server| {
                                            view! {
                                                <McpServerCard
                                                    server=server
                                                    servers=servers
                                                    loading=loading
                                                    error=error
                                                    editing_server=editing_server
                                                    show_dialog=show_dialog
                                                />
                                            }
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                // Info Box
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-info text-sm">"ℹ️"</span>
                        <span class="text-sm text-info">
                            "MCP servers provide external tools and integrations. Configure servers with their command, arguments, and environment variables."
                        </span>
                    </div>
                </div>
            </div>

            // Edit/Add Dialog
            <Show when=move || show_dialog.get()>
                <EditMcpServerDialog
                    editing_server=editing_server
                    on_close=move || show_dialog.set(false)
                    servers=servers
                    loading=loading
                    error=error
                />
            </Show>
        </div>
    }
}

#[component]
fn McpServerCard(
    server: McpServerInfo,
    servers: RwSignal<Vec<McpServerInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    editing_server: RwSignal<Option<String>>,
    show_dialog: RwSignal<bool>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let deleting = RwSignal::new(false);
    let server_name = StoredValue::new(server.name.clone());

    let cmd_summary = if server.args.is_empty() {
        server.command.clone()
    } else {
        format!("{} {}", server.command, server.args.join(" "))
    };

    let env_count = server.env.as_ref().map(|e| e.len()).unwrap_or(0);

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-primary-subtle flex items-center justify-center flex-shrink-0">
                        <span class="text-primary">"🔧"</span>
                    </div>
                    <div>
                        <div class="flex items-center gap-2">
                            <span class="text-sm font-medium text-text-primary">
                                {server.name}
                            </span>
                            <span class=move || {
                                if server.enabled {
                                    "px-2 py-0.5 rounded text-xs bg-success-subtle text-success"
                                } else {
                                    "px-2 py-0.5 rounded text-xs bg-surface-sunken text-text-tertiary"
                                }
                            }>
                                {if server.enabled { "Enabled" } else { "Disabled" }}
                            </span>
                        </div>
                        <p class="text-xs text-text-secondary mt-1 font-mono">
                            {cmd_summary}
                        </p>
                        {(env_count > 0).then(|| view! {
                            <div class="flex items-center gap-1 mt-2">
                                <span class="text-xs text-text-tertiary">"🔑"</span>
                                <span class="px-2 py-0.5 bg-surface-sunken border border-border rounded text-xs text-text-secondary">
                                    {format!("{} env var{}", env_count, if env_count != 1 { "s" } else { "" })}
                                </span>
                            </div>
                        })}
                    </div>
                </div>

                <div class="flex items-center gap-2 flex-shrink-0 ml-4">
                    <button
                        class="p-1.5 text-text-secondary hover:bg-surface-sunken rounded"
                        title="Edit"
                        on:click=move |_| {
                            editing_server.set(Some(server_name.get_value()));
                            show_dialog.set(true);
                        }
                    >
                        "✏️"
                    </button>
                    {move || {
                        if deleting.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    class="p-1.5 text-danger hover:bg-danger-subtle rounded"
                                    title="Delete"
                                    on:click=move |_| {
                                        deleting.set(true);
                                        let name = server_name.get_value();
                                        spawn_local(async move {
                                            match McpConfigApi::delete(&state, name).await {
                                                Ok(_) => {
                                                    load_servers(state, servers, loading, error);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to delete server: {}", e)));
                                                    deleting.set(false);
                                                }
                                            }
                                        });
                                    }
                                >
                                    "🗑️"
                                </button>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn EditMcpServerDialog(
    editing_server: RwSignal<Option<String>>,
    on_close: impl Fn() + 'static + Copy,
    servers: RwSignal<Vec<McpServerInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let name = RwSignal::new(String::new());
    let command = RwSignal::new(String::new());
    let args = RwSignal::new(String::new());
    let env = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);
    let is_new = editing_server.get().is_none();

    // Load server data when editing
    if let Some(server_name) = editing_server.get() {
        let state_clone = state;
        spawn_local(async move {
            match McpConfigApi::get(&state_clone, server_name).await {
                Ok(server) => {
                    name.set(server.name);
                    command.set(server.command);
                    args.set(server.args.join(" "));
                    if let Some(env_map) = server.env {
                        let env_str = env_map
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join("\n");
                        env.set(env_str);
                    }
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to load server: {}", e)));
                }
            }
        });
    }

    let handle_save = move |_| {
        let server_name = name.get().trim().to_string();
        let server_command = command.get().trim().to_string();
        if server_name.is_empty() || server_command.is_empty() {
            return;
        }

        let server_args: Vec<String> = args
            .get()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        let server_env = if env.get().trim().is_empty() {
            None
        } else {
            let mut env_map = HashMap::new();
            for line in env.get().lines() {
                if let Some((k, v)) = line.split_once('=') {
                    env_map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
            Some(env_map)
        };

        let config = McpServerConfig {
            command: server_command,
            args: server_args,
            env: server_env,
        };

        saving.set(true);
        dialog_error.set(None);

        spawn_local(async move {
            let result = if is_new {
                McpConfigApi::create(&state, server_name, config).await
            } else {
                McpConfigApi::update(&state, server_name, config).await
            };

            match result {
                Ok(_) => {
                    saving.set(false);
                    load_servers(state, servers, loading, error);
                    on_close();
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to save: {}", e)));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="bg-surface border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">
                    {if is_new { "Add MCP Server" } else { "Edit MCP Server" }}
                </h2>
                <p class="text-sm text-text-secondary mb-4">
                    {if is_new { "Configure a new MCP server connection" } else { "Update server configuration" }}
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Name"</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm disabled:opacity-50"
                            placeholder="my-server"
                            disabled=move || !is_new
                            value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Command"</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="e.g., npx, python, node"
                            value=move || command.get()
                            on:input=move |ev| command.set(event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Arguments"</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="e.g., -m mcp_server --port 3000"
                            value=move || args.get()
                            on:input=move |ev| args.set(event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Environment Variables"</label>
                        <textarea
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                            rows="4"
                            placeholder="KEY=value\nANOTHER_KEY=another_value"
                            prop:value=move || env.get()
                            on:input=move |ev| env.set(event_target_value(&ev))
                        />
                        <p class="text-xs text-text-tertiary mt-1">"One per line, format: KEY=value"</p>
                    </div>

                    {move || dialog_error.get().map(|err| view! {
                        <div class="flex items-center gap-2 text-danger text-sm">
                            <span>"⚠️"</span>
                            <span>{err}</span>
                        </div>
                    })}
                </div>

                <div class="flex gap-2 mt-6">
                    <button
                        class="flex-1 px-4 py-2 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                        on:click=move |_| on_close()
                    >
                        "Cancel"
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || name.get().trim().is_empty() || command.get().trim().is_empty() || saving.get()
                        on:click=handle_save
                    >
                        {move || if saving.get() { "Saving..." } else { "Save" }}
                    </button>
                </div>
            </div>
        </div>
    }
}
