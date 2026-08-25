//! Project room Workspace tab (P3, spec §6.4) — a read-only browse of the
//! directory the room is bound to.
//!
//! Deliberately NOT `DirectoryBrowser`. That component answers "which folder
//! do you want to bind", walking the whole host filesystem through `fs.*`;
//! this one answers "what is in the folder this room already chose", and its
//! server side refuses to leave the bound root. Reusing the picker here would
//! hand every room member a host-wide file browser — the two surfaces look
//! alike and mean opposite things.
//!
//! Three states this must keep distinct, because collapsing any two of them
//! tells the reader something untrue:
//!   * not bound      — a state, with an action (bind a folder in Settings)
//!   * bound + empty  — the folder really has nothing in it
//!   * refused        — the caller may not read this
//! The server keeps them apart on the wire (`root_bound`, an empty `entries`,
//! an error), so the only way to lose the distinction is here.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::projects::{ProjectInfo, ProjectsApi, WorkspaceEntry};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, Locale};
use leptos_i18n::I18nContext;

/// Join a breadcrumb trail into the `rel_path` the server expects.
///
/// Always `/`-separated regardless of host platform: this is a wire value,
/// and the server joins it onto a canonical root itself. Empty trail means
/// the root, which the server spells as an absent `rel_path`.
#[must_use]
pub(crate) fn rel_path_of(trail: &[String]) -> Option<String> {
    if trail.is_empty() {
        None
    } else {
        Some(trail.join("/"))
    }
}

/// Human-readable byte size. Directories are rendered without one — the
/// server sends `0` for them, and printing "0 B" next to a folder would state
/// a measurement nobody took.
#[must_use]
pub(crate) fn size_label(entry: &WorkspaceEntry) -> String {
    if entry.is_dir {
        return String::new();
    }
    let bytes = entry.size;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn load_failure(i18n: I18nContext<Locale>, err: &str) -> String {
    crate::components::admin_refusal::settings_load_error(i18n, err, |e| e.to_string())
}

#[component]
#[must_use]
pub fn WorkspaceTab(project: ProjectInfo) -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();

    let trail: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let entries: RwSignal<Vec<WorkspaceEntry>> = RwSignal::new(Vec::new());
    // `None` until the first answer arrives. Three-valued on purpose: `false`
    // means the server said unbound, `None` means nobody has said anything
    // yet, and rendering the bind prompt for the second would accuse an
    // unloaded tab of being unconfigured.
    let root_bound: RwSignal<Option<bool>> = RwSignal::new(None);
    let preview: RwSignal<Option<(String, String, bool)>> = RwSignal::new(None);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    let project_id = StoredValue::new(project.id.clone());

    let load = move || {
        let pid = project_id.get_value();
        let rel = rel_path_of(&trail.get());
        spawn_local(async move {
            match ProjectsApi::workspace_list(&dash, &pid, rel.as_deref()).await {
                Ok(listing) => {
                    root_bound.set(Some(listing.root_bound));
                    entries.set(listing.entries);
                    error.set(None);
                }
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
        });
    };

    // Re-runs on every trail change AND on reconnect: a tab mounted before the
    // socket was authorized would otherwise sit empty forever, which is
    // indistinguishable from an empty folder.
    Effect::new(move |_| {
        trail.track();
        if dash.is_connected.get() {
            load();
        }
    });

    let open_entry = move |entry: WorkspaceEntry| {
        if entry.is_dir {
            preview.set(None);
            trail.update(|t| t.push(entry.name.clone()));
            return;
        }
        let pid = project_id.get_value();
        let mut parts = trail.get_untracked();
        parts.push(entry.name.clone());
        let rel = parts.join("/");
        let title = entry.name.clone();
        spawn_local(async move {
            match ProjectsApi::workspace_read(&dash, &pid, &rel).await {
                Ok(p) => preview.set(Some((title, p.content, p.truncated))),
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
        });
    };

    view! {
        <div class="flex-1 overflow-auto px-6 py-4 space-y-3">
            <Show when=move || error.get().is_some()>
                <div class="rounded-md bg-danger-subtle px-3 py-2 text-xs text-danger">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <Show when=move || root_bound.get() == Some(false)>
                <p class="text-sm text-text-tertiary">
                    {t!(i18n, project_room.workspace_unbound)}
                </p>
            </Show>

            <Show when=move || root_bound.get() == Some(true)>
                <div class="flex items-center gap-1 text-xs text-text-secondary">
                    <button
                        class="rounded px-1.5 py-0.5 hover:bg-surface-hover"
                        on:click=move |_| { preview.set(None); trail.set(Vec::new()); }
                    >
                        {t!(i18n, project_room.workspace_root)}
                    </button>
                    {move || {
                        let parts = trail.get();
                        parts.iter().enumerate().map(|(i, name)| {
                            let depth = i + 1;
                            let label = name.clone();
                            view! {
                                <span class="text-text-tertiary">"/"</span>
                                <button
                                    class="rounded px-1.5 py-0.5 hover:bg-surface-hover"
                                    on:click=move |_| {
                                        preview.set(None);
                                        trail.update(|t| t.truncate(depth));
                                    }
                                >
                                    {label.clone()}
                                </button>
                            }
                        }).collect_view()
                    }}
                </div>

                <ul class="divide-y divide-border-subtle rounded-md border border-border-subtle">
                    <For
                        each=move || entries.get()
                        key=|e: &WorkspaceEntry| (e.name.clone(), e.is_dir)
                        let:entry
                    >
                        {
                            let for_click = entry.clone();
                            let size = size_label(&entry);
                            let icon = if entry.is_dir { "📁" } else { "📄" };
                            let name = entry.name.clone();
                            view! {
                                <li>
                                    <button
                                        class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-surface-hover"
                                        on:click=move |_| open_entry(for_click.clone())
                                    >
                                        <span>{icon}</span>
                                        <span class="flex-1 truncate">{name}</span>
                                        <span class="text-xs text-text-tertiary">{size}</span>
                                    </button>
                                </li>
                            }
                        }
                    </For>
                </ul>

                <Show when=move || entries.get().is_empty()>
                    <p class="text-sm text-text-tertiary">
                        {t!(i18n, project_room.workspace_empty_dir)}
                    </p>
                </Show>
            </Show>

            <Show when=move || preview.get().is_some()>
                <div class="rounded-md border border-border-subtle bg-surface-raised">
                    <div class="flex items-center justify-between border-b border-border-subtle px-3 py-2">
                        <span class="truncate text-xs font-medium">
                            {move || preview.get().map(|(t, _, _)| t).unwrap_or_default()}
                        </span>
                        <button
                            class="rounded px-2 py-0.5 text-xs hover:bg-surface-hover"
                            on:click=move |_| preview.set(None)
                        >
                            {t!(i18n, project_room.workspace_close_preview)}
                        </button>
                    </div>
                    <Show when=move || preview.get().is_some_and(|(_, _, cut)| cut)>
                        <div class="border-b border-border-subtle px-3 py-1.5 text-[11px] text-text-tertiary">
                            {t!(i18n, project_room.workspace_truncated)}
                        </div>
                    </Show>
                    <pre class="max-h-96 overflow-auto px-3 py-2 text-xs whitespace-pre-wrap break-words">
                        {move || preview.get().map(|(_, c, _)| c).unwrap_or_default()}
                    </pre>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> WorkspaceEntry {
        WorkspaceEntry {
            name: name.into(),
            is_dir: false,
            size,
        }
    }

    #[test]
    fn the_root_sends_no_rel_path_and_a_trail_joins_with_slashes() {
        assert_eq!(rel_path_of(&[]), None, "the root is an absent rel_path");
        assert_eq!(
            rel_path_of(&["src".to_string(), "views".to_string()]),
            Some("src/views".to_string()),
            "always /-separated: this is a wire value, not a host path"
        );
    }

    /// A directory gets no size label. The server sends `0` for one, and
    /// printing "0 B" beside a folder states a measurement nobody took.
    #[test]
    fn a_directory_carries_no_size_label() {
        let dir = WorkspaceEntry {
            name: "src".into(),
            is_dir: true,
            size: 0,
        };
        assert_eq!(size_label(&dir), "");
        assert_eq!(size_label(&file("a.txt", 0)), "0 B");
    }

    #[test]
    fn sizes_step_through_their_units() {
        assert_eq!(size_label(&file("a", 512)), "512 B");
        assert_eq!(size_label(&file("b", 2048)), "2.0 KB");
        assert_eq!(size_label(&file("c", 3 * 1024 * 1024)), "3.0 MB");
    }
}
