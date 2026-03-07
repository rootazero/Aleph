use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
}

/// Load plugins list from Gateway
fn load_plugins(
    state: DashboardState,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match state.rpc_call("plugins.list", json!({})).await {
            Ok(result) => {
                if let Some(list) = result.get("plugins") {
                    if let Ok(parsed) = serde_json::from_value::<Vec<PluginInfo>>(list.clone()) {
                        plugins.set(parsed);
                    }
                }
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load plugins: {}", e)));
                loading.set(false);
            }
        }
    });
}

#[component]
pub fn PluginsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let plugins = RwSignal::new(Vec::<PluginInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let auto_update = RwSignal::new(false);
    let show_install_dialog = RwSignal::new(false);

    // Load plugins when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_plugins(state, plugins, loading, error);
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
                            "Plugins"
                        </h1>
                        <p class="text-sm text-text-secondary">
                            "Extend Aleph with third-party plugins"
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| {
                                load_plugins(state, plugins, loading, error);
                            }
                        >
                            "Refresh"
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| show_install_dialog.set(true)
                        >
                            "+ Install Plugin"
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Settings Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Settings"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Auto Update"</div>
                                <div class="text-xs text-text-secondary mt-1">
                                    "Automatically update plugins when new versions are available"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || auto_update.get()
                                    on:change=move |ev| {
                                        auto_update.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>
                </div>

                // Installed Plugins Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">
                        {move || format!("Installed Plugins ({})", plugins.get().len())}
                    </h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                                </div>
                            }.into_any()
                        } else if plugins.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-border rounded">
                                    <div class="text-4xl mb-4">"🔌"</div>
                                    <p class="text-text-secondary">"No plugins installed"</p>
                                    <p class="text-xs text-text-tertiary mt-1">
                                        "Install plugins to extend Aleph's functionality"
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || plugins.get()
                                        key=|plugin| plugin.name.clone()
                                        children=move |plugin| {
                                            view! {
                                                <PluginCard
                                                    plugin=plugin
                                                    plugins=plugins
                                                    loading=loading
                                                    error=error
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
                            "Plugins can add new tools, integrations, and capabilities to Aleph. Install plugins from Git repositories, ZIP archives, or local folders."
                        </span>
                    </div>
                </div>
            </div>

            // Install Dialog
            <Show when=move || show_install_dialog.get()>
                <InstallPluginDialog
                    on_close=move || show_install_dialog.set(false)
                    plugins=plugins
                    loading=loading
                    error=error
                />
            </Show>
        </div>
    }
}

#[component]
fn PluginCard(
    plugin: PluginInfo,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let enabled = RwSignal::new(plugin.enabled);
    let deleting = RwSignal::new(false);
    let toggling = RwSignal::new(false);
    let plugin_name = StoredValue::new(plugin.name.clone());

    let description = if plugin.description.is_empty() {
        "No description".to_string()
    } else {
        plugin.description.clone()
    };

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-primary-subtle flex items-center justify-center flex-shrink-0">
                        <span class="text-primary">"🔌"</span>
                    </div>
                    <div>
                        <div class="flex items-center gap-2">
                            <span class="text-sm font-medium text-text-primary">
                                {plugin.name}
                            </span>
                            <span class="text-xs text-text-tertiary">
                                {format!("v{}", plugin.version)}
                            </span>
                        </div>
                        <p class="text-xs text-text-secondary mt-1">
                            {description}
                        </p>
                        <div class="flex items-center gap-1 mt-2 text-xs text-text-tertiary">
                            <span>"📦"</span>
                            <span>"Git Repository"</span>
                        </div>
                    </div>
                </div>

                <div class="flex items-center gap-2 flex-shrink-0 ml-4">
                    {move || {
                        if deleting.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    class="p-1.5 text-danger hover:bg-danger-subtle rounded"
                                    title="Remove"
                                    on:click=move |_| {
                                        deleting.set(true);
                                        let name = plugin_name.get_value();
                                        spawn_local(async move {
                                            match state.rpc_call("plugins.uninstall", json!({ "name": name })).await {
                                                Ok(_) => {
                                                    load_plugins(state, plugins, loading, error);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to delete plugin: {}", e)));
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
                    {move || {
                        if toggling.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <label class="relative inline-flex items-center cursor-pointer">
                                    <input
                                        type="checkbox"
                                        class="sr-only peer"
                                        checked=move || enabled.get()
                                        on:change=move |ev| {
                                            let new_val = event_target_checked(&ev);
                                            enabled.set(new_val);
                                            toggling.set(true);
                                            let name = plugin_name.get_value();
                                            let method = if new_val { "plugins.enable" } else { "plugins.disable" };
                                            spawn_local(async move {
                                                match state.rpc_call(method, json!({ "name": name })).await {
                                                    Ok(_) => {
                                                        toggling.set(false);
                                                    }
                                                    Err(e) => {
                                                        error.set(Some(format!("Failed to toggle plugin: {}", e)));
                                                        enabled.set(!new_val);
                                                        toggling.set(false);
                                                    }
                                                }
                                            });
                                        }
                                    />
                                    <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                                </label>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn InstallPluginDialog(
    on_close: impl Fn() + 'static + Copy,
    plugins: RwSignal<Vec<PluginInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let source = RwSignal::new("git".to_string());
    let url = RwSignal::new(String::new());
    let installing = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);

    let handle_install = move |_| {
        if url.get().trim().is_empty() {
            return;
        }
        installing.set(true);
        dialog_error.set(None);
        let install_url = url.get().trim().to_string();
        spawn_local(async move {
            match state.rpc_call("plugins.install", json!({
                "url": install_url,
            })).await {
                Ok(_) => {
                    installing.set(false);
                    load_plugins(state, plugins, loading, error);
                    on_close();
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to install: {}", e)));
                    installing.set(false);
                }
            }
        });
    };

    view! {
        <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="bg-surface border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">"Install Plugin"</h2>
                <p class="text-sm text-text-secondary mb-4">
                    "Install a plugin from Git repository, ZIP file, or local folder"
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Source"</label>
                        <select
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            on:change=move |ev| source.set(event_target_value(&ev))
                        >
                            <option value="git">"📦 Git Repository"</option>
                            <option value="zip">"📁 ZIP Archive"</option>
                            <option value="local">"💾 Local Folder"</option>
                        </select>
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">
                            {move || match source.get().as_str() {
                                "git" => "Repository URL",
                                "zip" => "ZIP URL or Path",
                                _ => "Folder Path",
                            }}
                        </label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder=move || match source.get().as_str() {
                                "git" => "https://github.com/user/plugin.git",
                                "zip" => "https://example.com/plugin.zip",
                                _ => "/path/to/plugin",
                            }
                            value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
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
                        disabled=move || url.get().trim().is_empty() || installing.get()
                        on:click=handle_install
                    >
                        {move || if installing.get() { "Installing..." } else { "Install" }}
                    </button>
                </div>
            </div>
        </div>
    }
}
