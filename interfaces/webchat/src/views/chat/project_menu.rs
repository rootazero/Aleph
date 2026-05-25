//! "进入项目工作 ▾" composer affordance.
//!
//! Renders a small pill below the textarea. When project mode is inactive
//! the pill says "进入项目工作 ▾" and opens a dropdown with three actions:
//! create a blank folder, pick an existing folder, or jump back to a
//! recent project. Once a project is active the pill shows its name with
//! a "退出" link so the user can leave project mode cleanly.
//!
//! The folder/parent picker is owned by the desktop shell (Tauri command
//! `pick_project_directory`); when the panel runs outside the shell (e.g.
//! a dev browser tab) the picker is unavailable and the pill falls back
//! to a path-input prompt.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::context::DashboardState;

use super::state::ChatState;

/// Top-level composer accessory. Renders the active-project chip or the
/// trigger pill that opens the picker dropdown.
#[component]
pub fn ProjectMenu() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let menu_open = RwSignal::new(false);
    let recents: RwSignal<Vec<ProjectInfo>> = RwSignal::new(Vec::new());
    let busy = RwSignal::new(false);
    let last_error: RwSignal<Option<String>> = RwSignal::new(None);

    // Refresh the recents list whenever the dropdown opens. Cheap RPC,
    // and we want fresh ordering after every project switch.
    Effect::new(move |_| {
        if !menu_open.get() {
            return;
        }
        let dash = dashboard;
        spawn_local(async move {
            match ProjectsApi::list(&dash).await {
                Ok(list) => recents.set(list),
                Err(e) => last_error.set(Some(e)),
            }
        });
    });

    let on_pick_existing = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        last_error.set(None);
        let dash = dashboard;
        spawn_local(async move {
            match pick_directory_via_shell(None).await {
                Ok(Some(path)) => match ProjectsApi::add(&dash, &path, None).await {
                    Ok(project) => {
                        chat.set_active_project(
                            Some(project.path.clone()),
                            Some(project.name.clone()),
                        );
                        menu_open.set(false);
                    }
                    Err(e) => last_error.set(Some(e)),
                },
                Ok(None) => {}
                Err(e) => last_error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let on_create_blank = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        last_error.set(None);
        let dash = dashboard;
        spawn_local(async move {
            let parent = match pick_directory_via_shell(None).await {
                Ok(Some(p)) => p,
                Ok(None) => {
                    busy.set(false);
                    return;
                }
                Err(e) => {
                    last_error.set(Some(e));
                    busy.set(false);
                    return;
                }
            };
            let name = match prompt_for_project_name() {
                Some(n) if !n.trim().is_empty() => n.trim().to_string(),
                _ => {
                    busy.set(false);
                    return;
                }
            };
            match ProjectsApi::create_blank(&dash, &parent, &name).await {
                Ok(project) => {
                    chat.set_active_project(
                        Some(project.path.clone()),
                        Some(project.name.clone()),
                    );
                    menu_open.set(false);
                }
                Err(e) => last_error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let on_exit_project = move |_| {
        chat.set_active_project(None, None);
        menu_open.set(false);
    };

    // Visible-project chip body (shown when project mode is active).
    let chip_body = move || {
        view! {
            <div class="inline-flex items-center gap-2 px-2 py-1 rounded-md bg-surface-sunken border border-border-subtle">
                <svg viewBox="0 0 16 16" class="w-3.5 h-3.5 text-primary" fill="none" stroke="currentColor" stroke-width="1.5">
                    <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                </svg>
                <span
                    class="font-medium text-text-primary truncate max-w-[160px]"
                    title=move || chat.active_project_root.get().unwrap_or_default()
                >
                    {move || chat.active_project_name.get().unwrap_or_else(|| "project".to_string())}
                </span>
                <button
                    type="button"
                    class="text-text-tertiary hover:text-text-primary px-1"
                    title="退出项目"
                    on:click=on_exit_project
                >
                    "×"
                </button>
            </div>
        }
    };

    let trigger_pill = move || {
        view! {
            <button
                type="button"
                class="inline-flex items-center gap-1 px-2 py-1 rounded-md border border-border-subtle text-text-secondary hover:bg-surface-sunken transition-colors"
                on:click=move |_| menu_open.update(|v| *v = !*v)
            >
                <svg viewBox="0 0 16 16" class="w-3.5 h-3.5" fill="none" stroke="currentColor" stroke-width="1.5">
                    <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                    <path d="M10.5 8.5v3M9 10h3" stroke-linecap="round" />
                </svg>
                <span>"进入项目工作"</span>
                <span class="text-text-tertiary">"▾"</span>
            </button>
        }
    };

    // Avoid `collect::<Vec<_>>()` inside the view! macro — turbofish on a
    // method call confuses the macro tokenizer. Compose the list outside.
    let recent_items = move || {
        let list = recents.get();
        list.into_iter().take(8).collect::<Vec<ProjectInfo>>()
    };

    view! {
        <div class="aleph-project-menu relative inline-flex items-center text-xs leading-tight">
            <Show
                when=move || chat.active_project_root.get().is_some()
                fallback=trigger_pill
            >
                {chip_body()}
            </Show>

            <Show when=move || menu_open.get()>
                // bottom-full + mb-1 → menu pops UPWARD from the trigger pill,
                // because the composer it sits above lives at the bottom of
                // the viewport and a top-full dropdown would clip below the
                // visible area.
                <div
                    class="absolute z-10 left-0 bottom-full mb-1 w-64 rounded-lg border border-border-subtle bg-surface-base shadow-lg py-1"
                    on:mouseleave=move |_| menu_open.set(false)
                >
                    <button
                        type="button"
                        class="w-full text-left px-3 py-2 flex items-center gap-2 text-sm hover:bg-surface-sunken disabled:opacity-50"
                        disabled=move || busy.get()
                        on:click=on_create_blank
                    >
                        <span class="text-text-tertiary">"+"</span>
                        <span>"新建空白项目"</span>
                    </button>
                    <button
                        type="button"
                        class="w-full text-left px-3 py-2 flex items-center gap-2 text-sm hover:bg-surface-sunken disabled:opacity-50"
                        disabled=move || busy.get()
                        on:click=on_pick_existing
                    >
                        <span class="text-text-tertiary">"\u{1F4C2}"</span>
                        <span>"使用现有文件夹"</span>
                    </button>
                    <Show when=move || !recents.get().is_empty()>
                        <div class="border-t border-border-subtle my-1"></div>
                        <div class="px-3 py-1 text-[10px] uppercase tracking-wide text-text-tertiary">
                            "最近项目"
                        </div>
                        <For
                            each=recent_items
                            key=|project| project.id.clone()
                            children=move |project: ProjectInfo| {
                                let proj_for_click = project.clone();
                                let dash_for_click = dashboard;
                                let label = project.name.clone();
                                let path = project.path.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="w-full text-left px-3 py-1.5 hover:bg-surface-sunken flex flex-col"
                                        on:click=move |_| {
                                            let proj = proj_for_click.clone();
                                            let dash = dash_for_click;
                                            let id = proj.id.clone();
                                            spawn_local(async move {
                                                let _ = ProjectsApi::touch(&dash, &id).await;
                                            });
                                            chat.set_active_project(
                                                Some(proj.path.clone()),
                                                Some(proj.name.clone()),
                                            );
                                            menu_open.set(false);
                                        }
                                    >
                                        <span class="text-sm text-text-primary truncate">{label}</span>
                                        <span class="text-[11px] text-text-tertiary truncate">{path}</span>
                                    </button>
                                }
                            }
                        />
                    </Show>
                    <Show when=move || last_error.get().is_some()>
                        <div class="px-3 py-2 text-xs text-danger">
                            {move || last_error.get().unwrap_or_default()}
                        </div>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

// ---------------------------------------------------------------------------
// JS bridge — `window.alephShell.*` is injected by the Tauri shell. When
// running outside the shell (browser tab) the methods are absent; we fall
// back to `window.prompt` so devs can still smoke-test the flow.
// ---------------------------------------------------------------------------

async fn pick_directory_via_shell(default_path: Option<String>) -> Result<Option<String>, String> {
    use js_sys::{Object, Reflect};

    let window = web_sys::window().ok_or_else(|| "no global window".to_string())?;
    let shell = Reflect::get(&window, &JsValue::from_str("alephShell"))
        .unwrap_or(JsValue::UNDEFINED);
    let pick_handle = if shell.is_undefined() || shell.is_null() {
        None
    } else {
        // Shell exists but may be an older build without pickDirectory —
        // treat that as "no native picker" rather than a hard error so the
        // user still gets the prompt fallback instead of a silent dropdown.
        match Reflect::get(&shell, &JsValue::from_str("pickDirectory")) {
            Ok(v) if !v.is_undefined() && !v.is_null() && v.is_function() => Some(v),
            _ => None,
        }
    };
    let Some(pick) = pick_handle else {
        return Ok(window
            .prompt_with_message_and_default(
                "Project folder absolute path:",
                default_path.as_deref().unwrap_or(""),
            )
            .ok()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()));
    };
    let opts = Object::new();
    if let Some(default) = default_path.as_deref() {
        let _ = Reflect::set(
            &opts,
            &JsValue::from_str("defaultPath"),
            &JsValue::from_str(default),
        );
    }
    let promise = js_sys::Function::from(pick)
        .call1(&shell, &opts)
        .map_err(|e| format!("pickDirectory call failed: {e:?}"))?;
    let promise = js_sys::Promise::from(promise);
    let resolved = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("pickDirectory rejected: {e:?}"))?;
    if resolved.is_null() || resolved.is_undefined() {
        return Ok(None);
    }
    Ok(resolved.as_string())
}

fn prompt_for_project_name() -> Option<String> {
    web_sys::window()?
        .prompt_with_message_and_default("New project name:", "untitled")
        .ok()
        .flatten()
}
