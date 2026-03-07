// Files Tab — workspace file browser with inline editor

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::agents::{AgentsApi, WorkspaceFile};
use crate::context::DashboardState;

#[component]
pub fn FilesTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);

    let files = RwSignal::new(Vec::<WorkspaceFile>::new());
    let selected_file = RwSignal::new(Option::<String>::None);
    let file_content = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);
    let show_create = RwSignal::new(false);
    let new_filename = RwSignal::new(String::new());

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
        let id = agent_id.get_value();
        let dash = state;
        spawn_local(async move {
            match AgentsApi::files_get(&dash, &id, &filename).await {
                Ok(content) => file_content.set(content),
                Err(e) => file_content.set(format!("Error loading file: {}", e)),
            }
        });
    };

    view! {
        <div class="flex gap-6 min-h-[400px]">
            // File list panel
            <div class="w-64 flex-shrink-0 bg-surface-raised border border-border rounded-xl overflow-hidden">
                <div class="p-3 border-b border-border flex items-center justify-between">
                    <h3 class="text-sm font-medium text-text-primary">"Files"</h3>
                    <button
                        on:click=move |_| show_create.update(|v| *v = !*v)
                        class="text-xs text-primary hover:text-primary-hover"
                    >
                        "+ New"
                    </button>
                </div>

                {move || show_create.get().then(|| view! {
                    <div class="p-2 border-b border-border flex gap-1">
                        <input
                            type="text"
                            placeholder="filename.md"
                            prop:value=move || new_filename.get()
                            on:input=move |ev| new_filename.set(event_target_value(&ev))
                            class="flex-1 px-2 py-1 bg-surface-sunken border border-border rounded text-xs text-text-primary"
                        />
                        <button
                            on:click=move |_| {
                                let filename = new_filename.get();
                                if filename.is_empty() { return; }
                                let id = agent_id.get_value();
                                let dash = state;
                                spawn_local(async move {
                                    match AgentsApi::files_set(&dash, &id, &filename, "").await {
                                        Ok(()) => {
                                            show_create.set(false);
                                            new_filename.set(String::new());
                                            reload_files();
                                            selected_file.set(Some(filename));
                                            file_content.set(String::new());
                                        }
                                        Err(e) => web_sys::console::error_1(&format!("Create failed: {e}").into()),
                                    }
                                });
                            }
                            class="px-2 py-1 bg-primary text-white rounded text-xs"
                        >
                            "OK"
                        </button>
                    </div>
                })}

                <div class="overflow-y-auto">
                    {move || {
                        if is_loading.get() {
                            view! { <div class="p-3 text-xs text-text-tertiary">"Loading..."</div> }.into_any()
                        } else {
                            let current_selected = selected_file.get();
                            view! {
                                <div>
                                    {files.get().into_iter().map(|f| {
                                        let fname = f.filename.clone();
                                        let fname_click = fname.clone();
                                        let is_sel = current_selected.as_ref() == Some(&fname);
                                        view! {
                                            <button
                                                on:click=move |_| select_file(fname_click.clone())
                                                class=move || {
                                                    if is_sel {
                                                        "w-full text-left px-3 py-2 text-sm bg-sidebar-active text-sidebar-accent"
                                                    } else {
                                                        "w-full text-left px-3 py-2 text-sm text-text-secondary hover:bg-sidebar-active/50"
                                                    }
                                                }
                                            >
                                                <div class="flex items-center gap-2">
                                                    <span class="truncate">{fname}</span>
                                                    {f.is_bootstrap.then(|| view! {
                                                        <span class="text-[10px] text-info bg-info/10 px-1 rounded">"boot"</span>
                                                    })}
                                                </div>
                                                <div class="text-[10px] text-text-tertiary mt-0.5">
                                                    {format!("{} bytes", f.size_bytes)}
                                                </div>
                                            </button>
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
                                    <button
                                        on:click=move |_| {
                                            let Some(filename) = selected_file.get() else { return };
                                            let id = agent_id.get_value();
                                            let dash = state;
                                            spawn_local(async move {
                                                if AgentsApi::files_delete(&dash, &id, &filename).await.is_ok() {
                                                    selected_file.set(None);
                                                    file_content.set(String::new());
                                                    reload_files();
                                                }
                                            });
                                        }
                                        class="text-xs text-danger hover:text-danger/80"
                                    >
                                        "Delete"
                                    </button>
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
                                            let Some(filename) = selected_file.get() else { return };
                                            is_saving.set(true);
                                            save_message.set(None);
                                            let id = agent_id.get_value();
                                            let content = file_content.get();
                                            let dash = state;
                                            spawn_local(async move {
                                                match AgentsApi::files_set(&dash, &id, &filename, &content).await {
                                                    Ok(()) => save_message.set(Some((true, "File saved".to_string()))),
                                                    Err(e) => save_message.set(Some((false, e))),
                                                }
                                                is_saving.set(false);
                                            });
                                        }
                                        disabled=move || is_saving.get()
                                        class="px-4 py-1.5 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50 text-sm"
                                    >
                                        {move || if is_saving.get() { "Saving..." } else { "Save" }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary text-sm">
                                "Select a file to edit"
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
