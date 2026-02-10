use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub enabled: bool,
}

#[component]
pub fn PluginsView() -> impl IntoView {
    let plugins = RwSignal::new(Vec::<PluginInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let auto_update = RwSignal::new(false);
    let show_install_dialog = RwSignal::new(false);

    // TODO: Load plugins from Gateway
    Effect::new(move || {
        loading.set(false);
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-slate-950">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-slate-100 mb-1">
                            "Plugins"
                        </h1>
                        <p class="text-sm text-slate-400">
                            "Extend Aleph with third-party plugins"
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-slate-800 text-slate-300 rounded hover:bg-slate-700 text-sm"
                            on:click=move |_| {
                                loading.set(true);
                                // TODO: Reload plugins
                                loading.set(false);
                            }
                        >
                            "↻ Refresh"
                        </button>
                        <button
                            class="px-3 py-1.5 bg-indigo-600 text-white rounded hover:bg-indigo-700 text-sm"
                            on:click=move |_| show_install_dialog.set(true)
                        >
                            "+ Install Plugin"
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-red-900/20 border border-red-800 rounded text-red-400 text-sm">
                        {err}
                    </div>
                })}

                // Settings Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-slate-200">"Settings"</h2>
                    <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-slate-200">"Auto Update"</div>
                                <div class="text-xs text-slate-400 mt-1">
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
                                <div class="w-11 h-6 bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-indigo-600"></div>
                            </label>
                        </div>
                    </div>
                </div>

                // Installed Plugins Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-slate-200">
                        {move || format!("Installed Plugins ({})", plugins.get().len())}
                    </h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-indigo-500"></div>
                                </div>
                            }.into_any()
                        } else if plugins.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-slate-700 rounded">
                                    <div class="text-4xl mb-4">"🔌"</div>
                                    <p class="text-slate-400">"No plugins installed"</p>
                                    <p class="text-xs text-slate-500 mt-1">
                                        "Install plugins to extend Aleph's functionality"
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || plugins.get()
                                        key=|plugin| plugin.id.clone()
                                        children=move |plugin| {
                                            view! {
                                                <PluginCard plugin=plugin />
                                            }
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                // Info Box
                <div class="p-4 bg-blue-900/20 border border-blue-800 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-blue-400 text-sm">"ℹ️"</span>
                        <span class="text-sm text-blue-300">
                            "Plugins can add new tools, integrations, and capabilities to Aleph. Install plugins from Git repositories, ZIP archives, or local folders."
                        </span>
                    </div>
                </div>
            </div>

            // Install Dialog
            <Show when=move || show_install_dialog.get()>
                <InstallPluginDialog
                    on_close=move || show_install_dialog.set(false)
                />
            </Show>
        </div>
    }
}

#[component]
fn PluginCard(plugin: PluginInfo) -> impl IntoView {
    let enabled = RwSignal::new(plugin.enabled);
    let toggling = RwSignal::new(false);

    view! {
        <div class="p-4 bg-slate-900 border border-slate-800 rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-indigo-900/30 flex items-center justify-center flex-shrink-0">
                        <span class="text-indigo-400">"🔌"</span>
                    </div>
                    <div>
                        <div class="flex items-center gap-2">
                            <span class="text-sm font-medium text-slate-200">
                                {plugin.name}
                            </span>
                            <span class="text-xs text-slate-500">
                                {format!("v{}", plugin.version)}
                            </span>
                        </div>
                        <p class="text-xs text-slate-400 mt-1">
                            {plugin.description.unwrap_or_else(|| "No description".to_string())}
                        </p>
                        <div class="flex items-center gap-1 mt-2 text-xs text-slate-500">
                            <span>"📦"</span>
                            <span>"Git Repository"</span>
                        </div>
                    </div>
                </div>

                <div class="flex items-center gap-2 flex-shrink-0 ml-4">
                    <button
                        class="p-1.5 text-red-400 hover:bg-red-900/20 rounded"
                        title="Remove"
                    >
                        "🗑️"
                    </button>
                    {move || {
                        if toggling.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-slate-400"></div>
                            }.into_any()
                        } else {
                            view! {
                                <label class="relative inline-flex items-center cursor-pointer">
                                    <input
                                        type="checkbox"
                                        class="sr-only peer"
                                        checked=move || enabled.get()
                                        on:change=move |ev| {
                                            enabled.set(event_target_checked(&ev));
                                            toggling.set(true);
                                            // TODO: Call Gateway API
                                            toggling.set(false);
                                        }
                                    />
                                    <div class="w-11 h-6 bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-indigo-600"></div>
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
fn InstallPluginDialog(on_close: impl Fn() + 'static + Copy) -> impl IntoView {
    let source = RwSignal::new("git".to_string());
    let url = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let handle_install = move |_| {
        if url.get().trim().is_empty() {
            return;
        }
        loading.set(true);
        error.set(None);
        // TODO: Call Gateway API
        loading.set(false);
        on_close();
    };

    view! {
        <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="bg-slate-900 border border-slate-700 rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-slate-100 mb-2">"Install Plugin"</h2>
                <p class="text-sm text-slate-400 mb-4">
                    "Install a plugin from Git repository, ZIP file, or local folder"
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-slate-300 mb-2">"Source"</label>
                        <select
                            class="w-full px-3 py-2 bg-slate-800 border border-slate-700 rounded text-slate-200 text-sm"
                            on:change=move |ev| source.set(event_target_value(&ev))
                        >
                            <option value="git">"📦 Git Repository"</option>
                            <option value="zip">"📁 ZIP Archive"</option>
                            <option value="local">"💾 Local Folder"</option>
                        </select>
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-slate-300 mb-2">
                            {move || match source.get().as_str() {
                                "git" => "Repository URL",
                                "zip" => "ZIP URL or Path",
                                _ => "Folder Path",
                            }}
                        </label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-slate-800 border border-slate-700 rounded text-slate-200 text-sm"
                            placeholder=move || match source.get().as_str() {
                                "git" => "https://github.com/user/plugin.git",
                                "zip" => "https://example.com/plugin.zip",
                                _ => "/path/to/plugin",
                            }
                            value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
                    </div>

                    {move || error.get().map(|err| view! {
                        <div class="flex items-center gap-2 text-red-400 text-sm">
                            <span>"⚠️"</span>
                            <span>{err}</span>
                        </div>
                    })}
                </div>

                <div class="flex gap-2 mt-6">
                    <button
                        class="flex-1 px-4 py-2 bg-slate-800 text-slate-300 rounded hover:bg-slate-700 text-sm"
                        on:click=move |_| on_close()
                    >
                        "Cancel"
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700 text-sm disabled:opacity-50"
                        disabled=move || url.get().trim().is_empty() || loading.get()
                        on:click=handle_install
                    >
                        {move || if loading.get() { "Installing..." } else { "Install" }}
                    </button>
                </div>
            </div>
        </div>
    }
}
