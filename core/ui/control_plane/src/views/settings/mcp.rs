//! MCP Configuration View
//!
//! Provides UI for managing MCP server configurations:
//! - List all MCP servers
//! - Add/Edit/Delete servers
//! - Configure command, args, and environment variables
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use std::collections::HashMap;

use crate::api::{McpConfigApi, McpServerConfig, McpServerInfo};
use crate::context::DashboardState;

#[component]
pub fn McpView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let servers = create_rw_signal(Vec::<McpServerInfo>::new());
    let selected = create_rw_signal(Option::<String>::None);
    let loading = create_rw_signal(true);
    let error = create_rw_signal(Option::<String>::None);

    // Load servers on mount
    create_effect(move |_| {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match McpConfigApi::list(&state).await {
                    Ok(list) => {
                        servers.set(list);
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load MCP servers: {}", e)));
                    }
                }
                loading.set(false);
            });
        }
    });

    // Subscribe to config change events
    create_effect(move |_| {
        // TODO: Subscribe to "config.mcp.**" events and reload servers
    });

    view! {
        <div class="flex h-full">
            <ServerList servers=servers selected=selected loading=loading error=error />
            <ServerEditor servers=servers selected=selected />
        </div>
    }
}

#[component]
fn ServerList(
    servers: RwSignal<Vec<McpServerInfo>>,
    selected: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let add_server = move |_| {
        selected.set(Some("__new__".to_string()));
    };

    view! {
        <div class="w-80 border-r border-gray-200 dark:border-gray-700 flex flex-col">
            <div class="p-4 border-b border-gray-200 dark:border-gray-700">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-lg font-semibold">"MCP Servers"</h2>
                    <button
                        on:click=add_server
                        class="px-3 py-1 bg-blue-500 text-white rounded hover:bg-blue-600"
                    >
                        "Add"
                    </button>
                </div>
            </div>

            <div class="flex-1 overflow-y-auto p-4">
                {move || {
                    if loading.get() {
                        view! { <div class="text-gray-500">"Loading..."</div> }.into_any()
                    } else if let Some(err) = error.get() {
                        view! { <div class="text-red-500">{err}</div> }.into_any()
                    } else {
                        let server_list = servers.get();
                        if server_list.is_empty() {
                            view! { <div class="text-gray-500">"No MCP servers configured"</div> }.into_any()
                        } else {
                            view! {
                                <div class="space-y-2">
                                    {move || {
                                        servers.get().into_iter().map(|server| {
                                            view! {
                                                <ServerCard
                                                    server=server
                                                    selected=selected
                                                />
                                            }
                                        }).collect::<Vec<_>>()
                                    }}
                                </div>
                            }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn ServerCard(
    server: McpServerInfo,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let name = server.name.clone();
    let name_for_memo = name.clone();
    let is_selected = create_memo(move |_| {
        selected.get().as_ref() == Some(&name_for_memo)
    });

    let select = move |_| {
        selected.set(Some(name.clone()));
    };

    view! {
        <div
            on:click=select
            class=move || {
                let base = "p-3 mb-2 rounded cursor-pointer border";
                if is_selected.get() {
                    format!("{} bg-blue-50 dark:bg-blue-900 border-blue-500", base)
                } else {
                    format!("{} bg-white dark:bg-gray-800 border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-700", base)
                }
            }
        >
            <div class="font-medium">{server.name.clone()}</div>
            <div class="text-sm text-gray-500 truncate">{server.command.clone()}</div>
        </div>
    }
}

#[component]
fn ServerEditor(
    servers: RwSignal<Vec<McpServerInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let name = create_rw_signal(String::new());
    let command = create_rw_signal(String::new());
    let args = create_rw_signal(String::new());
    let env = create_rw_signal(String::new());
    let saving = create_rw_signal(false);
    let error = create_rw_signal(Option::<String>::None);

    // Load server data when selection changes
    create_effect(move |_| {
        if let Some(selected_name) = selected.get() {
            if selected_name == "__new__" {
                name.set(String::new());
                command.set(String::new());
                args.set(String::new());
                env.set(String::new());
            } else {
                spawn_local(async move {
                    match McpConfigApi::get(&state, selected_name.clone()).await {
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
                            } else {
                                env.set(String::new());
                            }
                            error.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to load server: {}", e)));
                        }
                    }
                });
            }
        }
    });

    let save = move |_| {
        let is_new = selected.get().as_ref() == Some(&"__new__".to_string());
        let server_name = name.get();
        let server_command = command.get();
        let server_args: Vec<String> = args
            .get()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        let server_env = if env.get().is_empty() {
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

        spawn_local(async move {
            saving.set(true);
            let result = if is_new {
                McpConfigApi::create(&state, server_name.clone(), config).await
            } else {
                McpConfigApi::update(&state, server_name.clone(), config).await
            };

            match result {
                Ok(_) => {
                    // Reload servers
                    if let Ok(list) = McpConfigApi::list(&state).await {
                        servers.set(list);
                    }
                    if is_new {
                        selected.set(Some(server_name));
                    }
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    let delete = move |_| {
        if let Some(server_name) = selected.get() {
            if server_name != "__new__" {
                spawn_local(async move {
                    match McpConfigApi::delete(&state, server_name).await {
                        Ok(_) => {
                            // Reload servers
                            if let Ok(list) = McpConfigApi::list(&state).await {
                                servers.set(list);
                            }
                            selected.set(None);
                            error.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to delete: {}", e)));
                        }
                    }
                });
            }
        }
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto">
            {move || {
                if selected.get().is_none() {
                    view! {
                        <div class="text-gray-500 text-center mt-20">
                            "Select a server or click Add to create a new one"
                        </div>
                    }.into_any()
                } else {
                    let is_new = selected.get().as_ref() == Some(&"__new__".to_string());
                    view! {
                        <div class="max-w-2xl">
                            <h2 class="text-xl font-semibold mb-6">
                                {if is_new { "New MCP Server" } else { "Edit MCP Server" }}
                            </h2>

                            {move || error.get().map(|e| view! {
                                <div class="mb-4 p-3 bg-red-50 dark:bg-red-900 text-red-700 dark:text-red-200 rounded">
                                    {e}
                                </div>
                            })}

                            <div class="space-y-4">
                                <div>
                                    <label class="block text-sm font-medium mb-1">"Name"</label>
                                    <input
                                        type="text"
                                        prop:value=move || name.get()
                                        on:input=move |ev| name.set(event_target_value(&ev))
                                        prop:disabled=move || !is_new
                                        class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 disabled:opacity-50"
                                    />
                                </div>

                                <div>
                                    <label class="block text-sm font-medium mb-1">"Command"</label>
                                    <input
                                        type="text"
                                        prop:value=move || command.get()
                                        on:input=move |ev| command.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800"
                                        placeholder="e.g., npx, python, node"
                                    />
                                </div>

                                <div>
                                    <label class="block text-sm font-medium mb-1">"Arguments"</label>
                                    <input
                                        type="text"
                                        prop:value=move || args.get()
                                        on:input=move |ev| args.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800"
                                        placeholder="e.g., -m mcp_server"
                                    />
                                </div>

                                <div>
                                    <label class="block text-sm font-medium mb-1">"Environment Variables"</label>
                                    <textarea
                                        prop:value=move || env.get()
                                        on:input=move |ev| env.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 font-mono text-sm"
                                        rows="6"
                                        placeholder="KEY=value\nANOTHER_KEY=another_value"
                                    />
                                    <p class="text-xs text-gray-500 mt-1">"One per line, format: KEY=value"</p>
                                </div>

                                <div class="flex gap-2 pt-4">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-4 py-2 bg-blue-500 text-white rounded hover:bg-blue-600 disabled:opacity-50"
                                    >
                                        {move || if saving.get() { "Saving..." } else { "Save" }}
                                    </button>

                                    {move || {
                                        if !is_new {
                                            view! {
                                                <button
                                                    on:click=delete
                                                    class="px-4 py-2 bg-red-500 text-white rounded hover:bg-red-600"
                                                >
                                                    "Delete"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }
                                    }}
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

