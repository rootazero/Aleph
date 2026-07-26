//! Workspace files — the project tree, folded into the artifacts pane.
//!
//! This used to be `workspace_panel::FilesDrawer`, a separate bottom drawer
//! sitting under the (now deleted) inspector. It lives here so the pane has
//! **one** list model and the user never meets two different file browsers in
//! the same column. `components/directory_browser.rs` is a different thing
//! entirely — a modal picker for choosing a project root — and is untouched.
//!
//! State is component-local. The old drawer kept `files_drawer_open` and
//! `selected_file` on the shared `WorkspaceState`, which only ever had this one
//! consumer; after the inspector was deleted those fields were left with zero
//! consumers at all and were removed. Nothing outside this section needs to
//! know which folder is open, so nothing outside it holds that state.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::fs::{DirEntry, FsApi, ReadFileResult};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

/// A file the user opened, held for read-only preview.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePreview {
    path: String,
    content: String,
    truncated: bool,
}

/// Collapsible project-file section at the foot of the artifacts pane.
#[component]
pub(super) fn WorkspaceFiles() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let entries = RwSignal::new(Vec::<DirEntry>::new());
    let cur_path = RwSignal::new(Option::<String>::None);
    let parent = RwSignal::new(Option::<String>::None);
    let preview = RwSignal::new(Option::<FilePreview>::None);

    // Effect A — seed the path when the section opens. Prefer the active
    // project root, else the first allowed root. Skips reseeding once a path is
    // set so folder navigation isn't clobbered.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if cur_path.get().is_some() {
            return;
        }
        match chat.active_project_root.get() {
            Some(root) => cur_path.set(Some(root)),
            None => {
                let dash = dashboard;
                spawn_local(async move {
                    if let Ok(roots) = FsApi::allowed_roots(&dash).await {
                        if let Some(r) = roots.first() {
                            cur_path.set(Some(r.path.clone()));
                        }
                    }
                });
            }
        }
    });

    // Effect B — list entries whenever the path changes. Must NOT write
    // `cur_path` (only `entries`/`parent`), otherwise it would re-trigger
    // itself and fire a redundant `list_dir`.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(path) = cur_path.get() else {
            return;
        };
        let dash = dashboard;
        spawn_local(async move {
            if let Ok(listing) = FsApi::list_dir(&dash, &path, false).await {
                entries.set(listing.entries);
                // `None` means "you are at a root" — the server only reports a
                // parent that is itself inside an allowed root, so following it
                // can never walk out of the sandbox.
                parent.set(listing.parent);
            }
        });
    });

    // Reset navigation when the active project changes so a session switch
    // doesn't leave the previous project's listing behind. Reads
    // `active_project_root` only; writes the rest (never reads them) → cannot
    // self-retrigger. Effect A then reseeds from the new root.
    Effect::new(move |_| {
        let _ = chat.active_project_root.get();
        cur_path.set(None);
        entries.set(Vec::new());
        parent.set(None);
        preview.set(None);
    });

    view! {
        <div class="border-t border-border shrink-0">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-4 py-2 text-left text-[11px]
                       uppercase tracking-wider text-text-tertiary hover:text-text-secondary"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <span>{move || t_string!(i18n, common.workspace_files).to_string()}</span>
                <span class="ml-auto">{move || if open.get() { "▾" } else { "▸" }}</span>
            </button>

            <Show when=move || open.get()>
                <div class="flex max-h-[40vh] border-t border-border/60">
                    <div class="w-1/3 overflow-y-auto border-r border-border/60 p-2 text-xs">
                        {move || parent.get().map(|p| view! {
                            <button
                                type="button"
                                class="w-full text-left truncate px-1 py-0.5 rounded
                                       text-text-tertiary hover:bg-surface-raised/50"
                                on:click=move |_| cur_path.set(Some(p.clone()))
                            >"‹ .."</button>
                        })}
                        <For
                            each=move || entries.get()
                            key=|e| e.path.clone()
                            children=move |e: DirEntry| {
                                let path = e.path.clone();
                                let is_dir = e.is_dir;
                                view! {
                                    <button
                                        type="button"
                                        class="w-full text-left truncate px-1 py-0.5 rounded
                                               hover:bg-surface-raised/50"
                                        on:click=move |_| {
                                            if is_dir {
                                                cur_path.set(Some(path.clone()));
                                            } else {
                                                let dash = dashboard;
                                                let p = path.clone();
                                                spawn_local(async move {
                                                    if let Ok(ReadFileResult { path, content, truncated }) =
                                                        FsApi::read_file(&dash, &p).await
                                                    {
                                                        preview.set(Some(FilePreview {
                                                            path,
                                                            content,
                                                            truncated,
                                                        }));
                                                    }
                                                });
                                            }
                                        }
                                    >
                                        {if e.is_dir {
                                            format!("📁 {}", e.name)
                                        } else {
                                            format!("📄 {}", e.name)
                                        }}
                                    </button>
                                }
                            }
                        />
                    </div>
                    <div class="flex-1 overflow-auto p-2">
                        {move || match preview.get() {
                            Some(f) => view! {
                                <div class="flex flex-col gap-1">
                                    <div class="text-[11px] font-mono text-text-tertiary truncate">
                                        {f.path.clone()}
                                        {if f.truncated { " (truncated)" } else { "" }}
                                    </div>
                                    <pre class="text-xs whitespace-pre-wrap break-words font-mono
                                                text-text-secondary">{f.content}</pre>
                                </div>
                            }
                            .into_any(),
                            None => view! {
                                <p class="text-xs text-text-tertiary italic">
                                    {t!(i18n, common.workspace_files_hint)}
                                </p>
                            }
                            .into_any(),
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}
