//! "enter project workspace ▾" composer affordance.
//!
//! Renders a small pill above the textarea. When project mode is inactive
//! the pill says "enter project workspace ▾" and opens a dropdown with two actions:
//! use an existing folder, or create a blank one. Both flows funnel into
//! the cross-platform [`DirectoryBrowser`] (which talks to the server's
//! `fs.*` RPCs) so the directory the user picks is **the server's** —
//! correct for desktop, localhost web, and remote Tailnet web alike.
//!
//! Once a project is active the pill shows its name with a "×" so the
//! user can leave project mode cleanly.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::components::directory_browser::DirectoryBrowser;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::WorkspaceState;

use super::state::ChatState;

/// Which user action a `DirectoryBrowser` open belongs to. Drives the
/// modal title and the post-pick callback (`add` vs `create_blank`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserPurpose {
    PickExisting,
    NewBlank,
}

/// Top-level composer accessory. Renders the active-project chip or the
/// trigger pill that opens the picker dropdown.
#[component]
#[must_use]
pub fn ProjectMenu() -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    // Entering / leaving / switching a project clears the chat session
    // (see `ChatState::set_active_project`), so the workspace pane's
    // tool-detail view and captured payloads must be evicted alongside it.
    let workspace = use_context::<WorkspaceState>();

    let menu_open = RwSignal::new(false);
    let recents: RwSignal<Vec<ProjectInfo>> = RwSignal::new(Vec::new());
    let last_error: RwSignal<Option<String>> = RwSignal::new(None);

    // DirectoryBrowser modal state. `purpose` decides what happens once
    // the user confirms a path — register existing, or mkdir + register.
    let browser_open = RwSignal::new(false);
    let purpose: RwSignal<BrowserPurpose> = RwSignal::new(BrowserPurpose::PickExisting);

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
                Err(e) => last_error.set(Some(
                    crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    });

    let open_browser = move |p: BrowserPurpose| {
        purpose.set(p);
        last_error.set(None);
        browser_open.set(true);
        menu_open.set(false);
    };

    // The DirectoryBrowser fires this with the path the user confirmed.
    // Both purposes resolve to the same action — register the folder and
    // enter it. "New blank" differs only in that the browser auto-opens
    // its inline "new subdirectory" input (see `auto_create` on the component
    // below), so the path we get back is the freshly created (and
    // navigated-into) folder. We deliberately avoid `window.prompt`: it is
    // silently disabled inside the Tauri webview, which previously left
    // "new blank" unable to enter the folder at all.
    let on_pick = Callback::new(move |path: String| {
        let dash = dashboard;
        spawn_local(async move {
            match ProjectsApi::add(&dash, &path, None).await {
                Ok(project) => {
                    chat.set_active_project(project.workspace_path.clone(), Some(project.name));
                    if let Some(ws) = workspace {
                        ws.reset();
                    }
                }
                Err(e) => last_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    });

    let on_exit_project = move |_| {
        chat.set_active_project(None, None);
        if let Some(ws) = workspace {
            ws.reset();
        }
        menu_open.set(false);
    };

    // Visible-project chip body (shown when project mode is active).
    let chip_body = move || {
        view! {
            <div class="inline-flex items-center gap-2 px-2 py-1 rounded-md bg-surface-sunken border border-border-subtle backdrop-blur-[var(--glass-blur-chrome)]">
                <svg viewBox="0 0 16 16" class="w-3.5 h-3.5 text-primary" fill="none" stroke="currentColor" stroke-width="1.5">
                    <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                </svg>
                <span
                    class="font-medium text-text-primary truncate max-w-[160px]"
                    title=move || chat.active_project_root.get().unwrap_or_default()
                >
                    {move || chat.active_project_name.get().unwrap_or_else(|| t_string!(i18n, chat.project_default_name).to_string())}
                </span>
                <button
                    type="button"
                    class="text-text-tertiary hover:text-text-primary px-1"
                    title=move || t_string!(i18n, chat.project_exit).to_string()
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
                class="inline-flex items-center gap-1 px-2 py-1 rounded-md border border-border-subtle bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)] text-text-secondary hover:bg-surface-sunken transition-colors"
                on:click=move |_| menu_open.update(|v| *v = !*v)
            >
                <svg viewBox="0 0 16 16" class="w-3.5 h-3.5" fill="none" stroke="currentColor" stroke-width="1.5">
                    <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                    <path d="M10.5 8.5v3M9 10h3" stroke-linecap="round" />
                </svg>
                <span>{t!(i18n, chat.project_enter)}</span>
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

    let modal_title = move || match purpose.get() {
        BrowserPurpose::PickExisting => {
            t_string!(i18n, chat.project_modal_pick_existing).to_string()
        }
        BrowserPurpose::NewBlank => t_string!(i18n, chat.project_modal_new_blank).to_string(),
    };
    let modal_confirm = move || match purpose.get() {
        BrowserPurpose::PickExisting => t_string!(i18n, chat.project_confirm_pick).to_string(),
        BrowserPurpose::NewBlank => t_string!(i18n, chat.project_confirm_new_here).to_string(),
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
                    class="glass absolute z-10 left-0 bottom-full mb-1 w-64 rounded-lg border border-border bg-surface-overlay/85 shadow-xl py-1"
                    on:mouseleave=move |_| menu_open.set(false)
                >
                    <button
                        type="button"
                        class="w-full text-left px-3 py-2 flex items-center gap-2 text-sm hover:bg-surface-sunken"
                        on:click=move |_| open_browser(BrowserPurpose::NewBlank)
                    >
                        <span class="text-text-tertiary">"+"</span>
                        <span>{t!(i18n, chat.project_new_blank)}</span>
                    </button>
                    <button
                        type="button"
                        class="w-full text-left px-3 py-2 flex items-center gap-2 text-sm hover:bg-surface-sunken"
                        on:click=move |_| open_browser(BrowserPurpose::PickExisting)
                    >
                        <span class="text-text-tertiary">"\u{1F4C2}"</span>
                        <span>{t!(i18n, chat.project_use_existing)}</span>
                    </button>
                    <Show when=move || !recents.get().is_empty()>
                        <div class="border-t border-border-subtle my-1"></div>
                        <div class="px-3 py-1 text-[10px] uppercase tracking-wide text-text-tertiary">
                            {t!(i18n, chat.project_recent_title)}
                        </div>
                        <For
                            each=recent_items
                            key=|project| project.id.clone()
                            children=move |project: ProjectInfo| {
                                let proj_for_click = project.clone();
                                let dash_for_click = dashboard;
                                let label = project.name.clone();
                                let path = project.workspace_path.clone().unwrap_or_default();
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
                                                proj.workspace_path.clone(),
                                                Some(proj.name),
                                            );
                                            if let Some(ws) = workspace {
                                                ws.reset();
                                            }
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

            // Cross-platform directory picker — opens for both
            // BrowserPurpose variants. `purpose` only drives the title /
            // confirm label and whether the browser auto-opens its inline
            // create-folder input; `on_pick` registers the path either way.
            <DirectoryBrowser
                open=browser_open
                on_pick=on_pick
                title=Signal::derive(modal_title)
                confirm_label=Signal::derive(modal_confirm)
                auto_create=Signal::derive(move || purpose.get() == BrowserPurpose::NewBlank)
            />
        </div>
    }
}
