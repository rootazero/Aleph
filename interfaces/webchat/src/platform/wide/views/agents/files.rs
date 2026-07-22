// Files Tab — fixed 5 identity files with inline editor

use crate::api::agents::{AgentsApi, WorkspaceFile};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The 5 canonical identity files, displayed in this order.
/// MEMORY.md is owned by the curated memory module, not loaded as an identity file.
const IDENTITY_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "HEARTBEAT.md",
];

#[component]
#[must_use]
pub fn FilesTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let agent_id = StoredValue::new(agent_id);

    let files = RwSignal::new(Vec::<WorkspaceFile>::new());
    let selected_file = RwSignal::new(Option::<String>::None);
    let file_content = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);
    let load_error = RwSignal::new(false);

    // Reload file list
    let reload_files = move || {
        let id = agent_id.get_value();
        let dash = state;
        spawn_local(async move {
            match AgentsApi::files_list(&dash, &id).await {
                Ok(resp) => files.set(resp.files),
                Err(e) => web_sys::console::error_1(&format!("Failed to list files: {e}").into()),
            }
            is_loading.set(false);
        });
    };

    Effect::new(move || {
        if state.is_connected.get() {
            reload_files();
        }
    });

    // Select and load file content
    let select_file = move |filename: String| {
        selected_file.set(Some(filename.clone()));
        save_message.set(None);
        load_error.set(false);
        let id = agent_id.get_value();
        let dash = state;
        spawn_local(async move {
            match AgentsApi::files_get(&dash, &id, &filename).await {
                Ok(content) => {
                    file_content.set(content);
                    load_error.set(false);
                }
                Err(_e) => {
                    file_content.set(String::new());
                    load_error.set(true);
                    save_message.set(Some((false, t_string!(i18n, common.error).to_string())));
                }
            }
        });
    };

    // Create a missing file with empty content, then select it
    let create_file = move |filename: String| {
        let id = agent_id.get_value();
        let dash = state;
        spawn_local(async move {
            match AgentsApi::files_set(&dash, &id, &filename, "").await {
                Ok(()) => {
                    reload_files();
                    selected_file.set(Some(filename));
                    file_content.set(String::new());
                    load_error.set(false);
                }
                Err(e) => web_sys::console::error_1(&format!("Create failed: {e}").into()),
            }
        });
    };

    view! {
        <div class="flex gap-6 min-h-[400px]">
            // File list panel — fixed 5 identity files
            <div class="w-64 flex-shrink-0 bg-surface-raised border border-border rounded-xl overflow-hidden">
                <div class="p-3 border-b border-border">
                    <h3 class="text-sm font-medium text-text-primary">{t!(i18n, agents.files.identity_files)}</h3>
                </div>

                <div class="overflow-y-auto">
                    {move || {
                        if is_loading.get() {
                            view! { <div class="p-3 text-xs text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else {
                            let current_files = files.get();
                            let current_selected = selected_file.get();
                            view! {
                                <div>
                                    {IDENTITY_FILES.iter().map(|&name| {
                                        let existing = current_files.iter().find(|f| f.filename == name);
                                        let is_missing = existing.is_none();
                                        let size = existing.map(|f| f.size_bytes).unwrap_or(0);
                                        let is_sel = current_selected.as_deref() == Some(name);
                                        let name_owned = name.to_string();
                                        let name_click = name_owned.clone();
                                        let name_create = name_owned.clone();
                                        view! {
                                            <div class=move || {
                                                if is_sel {
                                                    "w-full px-3 py-2 bg-sidebar-active"
                                                } else {
                                                    "w-full px-3 py-2 hover:bg-sidebar-active/50"
                                                }
                                            }>
                                                <div class="flex items-center justify-between">
                                                    {if is_missing {
                                                        view! {
                                                            <span class="text-sm text-text-tertiary">{name_owned}</span>
                                                            <button
                                                                on:click=move |_| create_file(name_create.clone())
                                                                class="text-[10px] text-primary hover:text-primary-hover bg-primary/10 px-1.5 py-0.5 rounded cursor-pointer"
                                                            >
                                                                {t!(i18n, agents.files.create)}
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <button
                                                                on:click=move |_| select_file(name_click.clone())
                                                                class="text-left flex-1"
                                                            >
                                                                <span class=move || {
                                                                    if is_sel { "text-sm text-sidebar-accent" } else { "text-sm text-text-secondary" }
                                                                }>{name_owned}</span>
                                                            </button>
                                                            <span class="text-[10px] text-text-tertiary">
                                                                {format!("{size} B")}
                                                            </span>
                                                        }.into_any()
                                                    }}
                                                </div>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Editor panel
            <div class="flex-1 flex flex-col bg-surface-raised border border-border rounded-xl overflow-hidden">
                {move || {
                    if selected_file.get().is_some() {
                        view! {
                            <div class="flex-1 flex flex-col">
                                <div class="p-3 border-b border-border flex items-center justify-between">
                                    <span class="text-sm font-medium text-text-primary font-mono">
                                        {move || selected_file.get().unwrap_or_default()}
                                    </span>
                                </div>
                                <textarea
                                    prop:value=move || file_content.get()
                                    on:input=move |ev| file_content.set(event_target_value(&ev))
                                    class="flex-1 p-4 bg-surface-sunken text-text-primary font-mono text-sm resize-none focus:outline-none"
                                    spellcheck="false"
                                />
                                {move || save_message.get().map(|(ok, msg)| {
                                    let cls = if ok {
                                        "px-3 py-2 text-sm text-success bg-success-subtle"
                                    } else {
                                        "px-3 py-2 text-sm text-danger bg-danger-subtle"
                                    };
                                    view! { <div class=cls>{msg}</div> }
                                })}
                                <div class="p-3 border-t border-border flex justify-end">
                                    <button
                                        on:click=move |_| {
                                            if load_error.get() { return; }
                                            let Some(filename) = selected_file.get() else { return };
                                            is_saving.set(true);
                                            save_message.set(None);
                                            let id = agent_id.get_value();
                                            let content = file_content.get();
                                            let dash = state;
                                            spawn_local(async move {
                                                match AgentsApi::files_set(&dash, &id, &filename, &content).await {
                                                    Ok(()) => save_message.set(Some((true, t_string!(i18n, agents.files.saved).to_string()))),
                                                    Err(e) => save_message.set(Some((false, e))),
                                                }
                                                is_saving.set(false);
                                            });
                                        }
                                        disabled=move || is_saving.get() || load_error.get()
                                        class="px-4 py-1.5 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50 text-sm"
                                    >
                                        {move || if is_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary text-sm">
                                {t!(i18n, agents.files.select_to_edit)}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
