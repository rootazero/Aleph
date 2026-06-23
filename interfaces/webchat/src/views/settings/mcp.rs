//! MCP Configuration View
//!
//! Provides UI for managing MCP server configurations:
//! - List all MCP servers as cards
//! - Add/Edit/Delete servers via dialog
//! - Configure command, args, and environment variables

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::{
    McpConfigApi, McpPresetApi, McpPresetEnvVar, McpPresetInfo, McpServerConfig, McpServerInfo,
    PresetInstallOutcome,
};
use crate::components::ui::SecretInput;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// One editable environment-variable row with a stable id for keyed iteration.
#[derive(Clone, Copy)]
struct EnvRow {
    id: usize,
    key: RwSignal<String>,
    value: RwSignal<String>,
    /// True if this secret was already configured on the server (loaded with its
    /// value redacted). Drives the "saved — blank keeps it" placeholder.
    configured: bool,
}

/// Heuristic: does this env var name look like it holds a secret?
/// Drives whether the value field is masked (provider-grade key UX).
fn is_secret_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "SECRET", "TOKEN", "PASSWORD", "PASS", "CREDENTIAL"]
        .iter()
        .any(|needle| upper.contains(needle))
}

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
                error.set(Some(format!("Failed to load MCP servers: {e}")));
                loading.set(false);
            }
        }
    });
}

/// Load the recommended-preset catalog (best-effort; failures are silent so the
/// configured-servers list still renders).
fn load_presets(state: DashboardState, presets: RwSignal<Vec<McpPresetInfo>>) {
    spawn_local(async move {
        if let Ok(list) = McpPresetApi::list(&state).await {
            presets.set(list);
        }
    });
}

#[component]
#[must_use]
pub fn McpView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let servers = RwSignal::new(Vec::<McpServerInfo>::new());
    let presets = RwSignal::new(Vec::<McpPresetInfo>::new());
    let installing_preset = RwSignal::new(Option::<McpPresetInfo>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_dialog = RwSignal::new(false);
    let editing_server = RwSignal::new(Option::<String>::None);

    // Load servers + presets when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_servers(state, servers, loading, error);
            load_presets(state, presets);
        } else {
            loading.set(false);
        }
    });

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary mb-1">
                            {t!(i18n, settings.mcp.title)}
                        </h1>
                        <p class="text-sm text-text-secondary">
                            {t!(i18n, settings.mcp.description)}
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| {
                                load_servers(state, servers, loading, error);
                            }
                        >
                            {t!(i18n, settings.mcp.refresh)}
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| {
                                editing_server.set(None);
                                show_dialog.set(true);
                            }
                        >
                            {t!(i18n, settings.mcp.add_server)}
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
                        {format!("{} ({})", t_string!(i18n, settings.mcp.configured_servers), servers.get().len())}
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
                                    <p class="text-text-secondary">{t!(i18n, settings.mcp.no_servers)}</p>
                                    <p class="text-xs text-text-tertiary mt-1">
                                        {t!(i18n, settings.mcp.no_servers_hint)}
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || servers.get()
                                        key=|server| server.id.clone()
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

                // Recommended Presets Section — surfaces catalog presets not yet
                // installed, so the user can configure keys and enable in one click.
                {move || {
                    let available: Vec<McpPresetInfo> =
                        presets.get().into_iter().filter(|p| !p.installed).collect();
                    (!available.is_empty()).then(|| view! {
                        <div class="space-y-4">
                            <div>
                                <h2 class="text-lg font-medium text-text-primary">
                                    {t!(i18n, settings.mcp.recommended)}
                                </h2>
                                <p class="text-xs text-text-tertiary mt-1">
                                    {t!(i18n, settings.mcp.recommended_hint)}
                                </p>
                            </div>
                            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                                {available.into_iter().map(|preset| {
                                    let p_for_click = preset.clone();
                                    view! {
                                        <div class="p-4 bg-surface-raised border border-border rounded flex flex-col gap-2">
                                            <div class="flex items-center gap-2">
                                                <span class="text-sm font-medium text-text-primary">
                                                    {preset.name.clone()}
                                                </span>
                                                {preset.official.then(|| view! {
                                                    <span class="px-2 py-0.5 rounded text-xs bg-primary-subtle text-primary">
                                                        {t_string!(i18n, settings.mcp.preset_official).to_string()}
                                                    </span>
                                                })}
                                            </div>
                                            <p class="text-xs text-text-secondary">
                                                {preset.description.clone()}
                                            </p>
                                            <div class="flex items-center justify-between mt-1">
                                                <span class="text-xs text-text-tertiary">
                                                    {preset.vendor.clone()}
                                                </span>
                                                <button
                                                    class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-xs"
                                                    on:click=move |_| installing_preset.set(Some(p_for_click.clone()))
                                                >
                                                    {t!(i18n, settings.mcp.preset_configure)}
                                                </button>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    })
                }}

                // Info Box
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-info text-sm">"ℹ️"</span>
                        <span class="text-sm text-info">
                            {t!(i18n, settings.mcp.info_text)}
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

            // Preset Install Dialog
            <Show when=move || installing_preset.get().is_some()>
                {move || installing_preset.get().map(|preset| view! {
                    <InstallPresetDialog
                        preset=preset
                        on_close=move || installing_preset.set(None)
                        servers=servers
                        presets=presets
                        loading=loading
                        error=error
                    />
                })}
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
    let i18n = use_i18n();
    let deleting = RwSignal::new(false);
    let server_id = StoredValue::new(server.id.clone());

    let cmd_summary = if server.args.is_empty() {
        server.command.clone()
    } else {
        format!("{} {}", server.command, server.args.join(" "))
    };

    let env_count = server
        .env
        .as_ref()
        .map(std::collections::HashMap::len)
        .unwrap_or(0);

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
                                {if server.enabled { t_string!(i18n, settings.mcp.enabled).to_string() } else { t_string!(i18n, settings.mcp.disabled).to_string() }}
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
                            editing_server.set(Some(server_id.get_value()));
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
                                        let id = server_id.get_value();
                                        spawn_local(async move {
                                            match McpConfigApi::delete(&state, id).await {
                                                Ok(_) => {
                                                    load_servers(state, servers, loading, error);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to delete server: {e}")));
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
    let i18n = use_i18n();
    let name = RwSignal::new(String::new());
    let command = RwSignal::new(String::new());
    let args = RwSignal::new(String::new());
    let env_rows = RwSignal::new(Vec::<EnvRow>::new());
    let next_env_id = StoredValue::new(0usize);
    let saving = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);
    let is_new = editing_server.get().is_none();

    // Load server data when editing
    if let Some(server_id) = editing_server.get() {
        let state_clone = state;
        spawn_local(async move {
            match McpConfigApi::get(&state_clone, server_id).await {
                Ok(server) => {
                    name.set(server.name);
                    command.set(server.command);
                    args.set(server.args.join(" "));
                    if let Some(env_map) = server.env {
                        // Sort for stable ordering (HashMap iteration is unordered).
                        let mut entries: Vec<(String, String)> = env_map.into_iter().collect();
                        entries.sort_by(|a, b| a.0.cmp(&b.0));
                        let mut id = next_env_id.get_value();
                        let rows: Vec<EnvRow> = entries
                            .into_iter()
                            .map(|(k, v)| {
                                let configured = is_secret_env_key(&k);
                                let row = EnvRow {
                                    id,
                                    configured,
                                    key: RwSignal::new(k),
                                    value: RwSignal::new(v),
                                };
                                id += 1;
                                row
                            })
                            .collect();
                        next_env_id.set_value(id);
                        env_rows.set(rows);
                    }
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to load server: {e}")));
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
            .map(std::string::ToString::to_string)
            .collect();

        let server_env = {
            let mut env_map = HashMap::new();
            for row in env_rows.get() {
                let k = row.key.get().trim().to_string();
                if k.is_empty() {
                    continue;
                }
                env_map.insert(k, row.value.get().trim().to_string());
            }
            if env_map.is_empty() {
                None
            } else {
                Some(env_map)
            }
        };

        let config = McpServerConfig {
            command: server_command,
            args: server_args,
            env: server_env,
        };

        saving.set(true);
        dialog_error.set(None);

        let editing_id = editing_server.get(); // Some(id) when editing, None when new
        spawn_local(async move {
            let result = if is_new {
                McpConfigApi::create(&state, server_name, config).await
            } else {
                let id = editing_id.unwrap_or_default();
                McpConfigApi::update(&state, id, config).await
            };

            match result {
                Ok(_) => {
                    saving.set(false);
                    load_servers(state, servers, loading, error);
                    on_close();
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to save: {e}")));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="glass bg-surface-overlay/85 border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">
                    {if is_new { t_string!(i18n, settings.mcp.add_mcp_server).to_string() } else { t_string!(i18n, settings.mcp.edit_mcp_server).to_string() }}
                </h2>
                <p class="text-sm text-text-secondary mb-4">
                    {if is_new { t_string!(i18n, settings.mcp.add_mcp_desc).to_string() } else { t_string!(i18n, settings.mcp.edit_mcp_desc).to_string() }}
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">{t!(i18n, settings.mcp.name_label)}</label>
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
                        <label class="block text-sm font-medium text-text-secondary mb-2">{t!(i18n, settings.mcp.command_label)}</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="e.g., npx, python, node"
                            value=move || command.get()
                            on:input=move |ev| command.set(event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">{t!(i18n, settings.mcp.args_label)}</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="e.g., -m mcp_server --port 3000"
                            value=move || args.get()
                            on:input=move |ev| args.set(event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">{t!(i18n, settings.mcp.env_label)}</label>
                        <div class="space-y-2">
                            <For
                                each=move || env_rows.get()
                                key=|row| row.id
                                children=move |row| {
                                    let key_sig = row.key;
                                    let val_sig = row.value;
                                    let configured = row.configured;
                                    view! {
                                        <div class="flex items-center gap-2">
                                            <input
                                                type="text"
                                                class="w-2/5 px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                                                placeholder=t_string!(i18n, settings.mcp.env_key_placeholder).to_string()
                                                value=move || key_sig.get()
                                                on:input=move |ev| key_sig.set(event_target_value(&ev))
                                            />
                                            <div class="flex-1">
                                                {move || {
                                                    if is_secret_env_key(&key_sig.get()) {
                                                        view! {
                                                            <SecretInput
                                                                value=val_sig.into()
                                                                on_change=move |v| val_sig.set(v)
                                                                placeholder=if configured {
                                                                    t_string!(i18n, settings.mcp.env_value_saved).to_string()
                                                                } else {
                                                                    t_string!(i18n, settings.mcp.env_value_placeholder).to_string()
                                                                }
                                                                monospace=true
                                                            />
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <input
                                                                type="text"
                                                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                                                                placeholder=t_string!(i18n, settings.mcp.env_value_placeholder).to_string()
                                                                value=move || val_sig.get()
                                                                on:input=move |ev| val_sig.set(event_target_value(&ev))
                                                            />
                                                        }.into_any()
                                                    }
                                                }}
                                            </div>
                                            <button
                                                type="button"
                                                class="p-1.5 text-danger hover:bg-danger-subtle rounded flex-shrink-0"
                                                title=t_string!(i18n, settings.mcp.env_remove).to_string()
                                                on:click=move |_| {
                                                    env_rows.update(|rows| rows.retain(|r| r.id != row.id));
                                                }
                                            >
                                                "🗑️"
                                            </button>
                                        </div>
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="text-sm text-primary hover:text-primary-hover"
                                on:click=move |_| {
                                    let id = next_env_id.get_value();
                                    next_env_id.set_value(id + 1);
                                    env_rows.update(|rows| {
                                        rows.push(EnvRow {
                                            id,
                                            configured: false,
                                            key: RwSignal::new(String::new()),
                                            value: RwSignal::new(String::new()),
                                        });
                                    });
                                }
                            >
                                {t!(i18n, settings.mcp.env_add)}
                            </button>
                        </div>
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.mcp.env_hint)}</p>
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
                        {t!(i18n, common.cancel)}
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || name.get().trim().is_empty() || command.get().trim().is_empty() || saving.get()
                        on:click=handle_save
                    >
                        {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Install a catalog preset: renders one key field per declared env var (secret
/// fields masked) and calls `mcp.install_preset`. On success the server becomes
/// a persisted config and shows up in the configured list.
#[component]
fn InstallPresetDialog(
    preset: McpPresetInfo,
    on_close: impl Fn() + 'static + Copy,
    servers: RwSignal<Vec<McpServerInfo>>,
    presets: RwSignal<Vec<McpPresetInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let saving = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);

    let preset_id = preset.id.clone();
    let preset_name = preset.name.clone();

    // One value signal per declared env var, prefilled with its default (if any).
    let rows: Vec<(McpPresetEnvVar, RwSignal<String>)> = preset
        .required_env
        .iter()
        .map(|e| (e.clone(), RwSignal::new(e.default.clone().unwrap_or_default())))
        .collect();
    let rows = StoredValue::new(rows);

    let handle_install = move |_| {
        let env: HashMap<String, String> = rows
            .get_value()
            .into_iter()
            .filter_map(|(ev, sig)| {
                let v = sig.get().trim().to_string();
                (!v.is_empty()).then_some((ev.key, v))
            })
            .collect();

        saving.set(true);
        dialog_error.set(None);
        let id = preset_id.clone();
        spawn_local(async move {
            match McpPresetApi::install(&state, id, env).await {
                Ok(PresetInstallOutcome::Installed | PresetInstallOutcome::AlreadyInstalled) => {
                    saving.set(false);
                    load_servers(state, servers, loading, error);
                    load_presets(state, presets);
                    on_close();
                }
                Ok(PresetInstallOutcome::NeedsKey(_)) => {
                    dialog_error
                        .set(Some(t_string!(i18n, settings.mcp.preset_needs_key).to_string()));
                    saving.set(false);
                }
                Ok(PresetInstallOutcome::NoRuntime(_)) => {
                    dialog_error
                        .set(Some(t_string!(i18n, settings.mcp.preset_no_runtime).to_string()));
                    saving.set(false);
                }
                Err(e) => {
                    dialog_error.set(Some(e));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="glass bg-surface-overlay/85 border border-border rounded-lg p-6 max-w-md w-full mx-4 max-h-[85vh] overflow-y-auto">
                <h2 class="text-lg font-semibold text-text-primary mb-1">
                    {t!(i18n, settings.mcp.preset_install_title)}
                </h2>
                <p class="text-sm text-text-secondary mb-4 font-medium">{preset_name}</p>

                <div class="space-y-4">
                    {rows.get_value().into_iter().map(|(ev, sig)| {
                        let secret = ev.secret;
                        let required = ev.required;
                        let label = if ev.label.is_empty() { ev.key.clone() } else { ev.label.clone() };
                        let desc = ev.description.clone();
                        let how_to = ev.how_to_get_url.clone();
                        let placeholder = ev.default.clone().unwrap_or_default();
                        view! {
                            <div>
                                <div class="flex items-center justify-between mb-1">
                                    <label class="block text-sm font-medium text-text-secondary">
                                        {label}
                                        {required.then(|| view! {
                                            <span class="text-danger ml-1">"*"</span>
                                        })}
                                    </label>
                                    {how_to.map(|url| view! {
                                        <a
                                            href=url
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            class="text-xs text-primary hover:text-primary-hover"
                                        >
                                            {t!(i18n, settings.mcp.preset_how_to_get)}
                                        </a>
                                    })}
                                </div>
                                {(!desc.is_empty()).then(|| view! {
                                    <p class="text-xs text-text-tertiary mb-1">{desc.clone()}</p>
                                })}
                                {move || {
                                    if secret {
                                        view! {
                                            <SecretInput
                                                value=sig.into()
                                                on_change=move |v| sig.set(v)
                                                placeholder=placeholder.clone()
                                                monospace=true
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <input
                                                type="text"
                                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                                                placeholder=placeholder.clone()
                                                value=move || sig.get()
                                                on:input=move |ev| sig.set(event_target_value(&ev))
                                            />
                                        }.into_any()
                                    }
                                }}
                            </div>
                        }
                    }).collect_view()}

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
                        {t!(i18n, settings.mcp.cancel)}
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || saving.get()
                        on:click=handle_install
                    >
                        {move || if saving.get() { t_string!(i18n, settings.mcp.preset_installing).to_string() } else { t_string!(i18n, settings.mcp.preset_install).to_string() }}
                    </button>
                </div>
            </div>
        </div>
    }
}
