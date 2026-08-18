//! Project room settings tab (P2 Task 8, spec §6.4) — roster, workspace
//! binding, rename, archive. Owner-only controls; a non-owner sees a
//! read-only view (mirrors the server's own `require_owner` gate — every
//! mutating RPC here is refused server-side for a plain member regardless of
//! what this component renders, but hiding the controls avoids a round trip
//! that can only ever come back `PERMISSION_DENIED`).
//!
//! Kanban / 工作区浏览 / 记忆浏览 are P3 — `ProjectRoomPage` (the parent)
//! renders those as bare placeholder tabs; this file is the 设置 tab body
//! only.
//!
//! Every mutation handler here is inlined directly at its `on:click=move
//! |_| { .. }` site rather than bound to a named closure first — `Show`'s
//! `children` (and any reactive `{move || ..}` block) must be re-callable
//! (`Fn`). IDs threaded into such a handler are held in `StoredValue<String>`
//! (Copy) rather than a plain `String` (not Copy): a plain `String` moved
//! into a `move` closure nested inside a `Fn`-required ancestor makes that
//! ancestor `FnOnce` — the ancestor cannot re-supply the same moved-out
//! `String` to a freshly-reconstructed child closure on a second call.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::ProjectsTabState;
use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::components::directory_browser::DirectoryBrowser;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::user_directory::UserDirectoryState;

/// `Ok(())` on success (the child re-fetches via `refresh`); `Err(message)`
/// surfaces in the shared error banner. One callback shape shared by every
/// mutation in this tab so the banner has a single source.
type OnDone = Callback<Result<(), String>>;

#[component]
#[must_use]
pub fn SettingsTab(project: ProjectInfo, refresh: Callback<()>) -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let dash = expect_context::<DashboardState>();
    let dir = expect_context::<UserDirectoryState>();
    let tab_state = expect_context::<ProjectsTabState>();

    dir.ensure_loaded(dash);

    // `Memo` (not a raw closure): needs to be `Copy` to pass to four child
    // components, and a closure capturing `owner: Option<String>` isn't.
    let is_owner: Memo<bool> = {
        let owner = project.owner_user_id.clone();
        Memo::new(move |_| dir.my_user_id.get().is_some() && dir.my_user_id.get() == owner)
    };

    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let on_done: OnDone = Callback::new(move |result: Result<(), String>| match result {
        Ok(()) => {
            error.set(None);
            refresh.run(());
        }
        Err(e) => error.set(Some(
            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| e.to_string()),
        )),
    });
    // Archiving is the one mutation that leaves the room, not just changes
    // it (spec §6.4: "archive returns to the project list") — clearing the
    // GLOBAL `selected_project_id` (not a per-conversation signal; see that
    // field's doc) is what `ProjectsView` reads to fall back to the
    // empty-state / list. A plain `refresh()` would instead re-fetch and
    // keep rendering the now-archived room in place.
    let on_archived: OnDone = Callback::new(move |result: Result<(), String>| match result {
        Ok(()) => tab_state.selected_project_id.set(None),
        Err(e) => error.set(Some(
            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| e.to_string()),
        )),
    });

    view! {
        <div class="max-w-xl mx-auto px-6 py-6 space-y-8">
            <Show when=move || error.get().is_some()>
                <div class="px-3 py-2 rounded-md bg-danger/10 text-danger text-sm">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <RenameSection project=project.clone() is_owner=is_owner dash=dash on_done=on_done />
            <RosterSection project=project.clone() is_owner=is_owner dash=dash dir=dir on_done=on_done />
            <WorkspaceSection project=project.clone() is_owner=is_owner dash=dash on_done=on_done />
            <ArchiveSection project=project.clone() is_owner=is_owner dash=dash on_done=on_archived />
        </div>
    }
}

#[component]
fn RenameSection(
    project: ProjectInfo,
    is_owner: Memo<bool>,
    dash: DashboardState,
    on_done: OnDone,
) -> impl IntoView {
    let i18n = use_i18n();
    let name = RwSignal::new(project.name.clone());
    let id = StoredValue::new(project.id.clone());
    let original = StoredValue::new(project.name.clone());
    view! {
        <section>
            <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-2">{t!(i18n, project_room.name)}</h3>
            <Show
                when=move || is_owner.get()
                fallback=move || view! { <p class="text-sm text-text-primary">{project.name.clone()}</p> }
            >
                <div class="flex items-center gap-2">
                    <input
                        type="text"
                        class="flex-1 min-w-0 px-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-primary/60"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                    />
                    <button
                        type="button"
                        class="px-3 py-1.5 rounded-md text-sm bg-primary/15 text-primary hover:bg-primary/25"
                        on:click=move |_| {
                            let id = id.get_value();
                            let new_name = name.get_untracked().trim().to_string();
                            if new_name.is_empty() || new_name == original.get_value() {
                                return;
                            }
                            spawn_local(async move {
                                let result = ProjectsApi::rename(&dash, &id, &new_name).await.map(|_| ());
                                on_done.run(result);
                            });
                        }
                    >
                        {t!(i18n, common.save_short)}
                    </button>
                </div>
            </Show>
        </section>
    }
}

#[component]
fn RosterSection(
    project: ProjectInfo,
    is_owner: Memo<bool>,
    dash: DashboardState,
    dir: UserDirectoryState,
    on_done: OnDone,
) -> impl IntoView {
    let i18n = use_i18n();
    let owner_id = project.owner_user_id.clone();
    let members = StoredValue::new(project.member_ids.clone());
    let project_id = StoredValue::new(project.id.clone());
    let picking = RwSignal::new(false);

    view! {
        <section>
            <div class="flex items-center justify-between mb-2">
                <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">{t!(i18n, project_room.members)}</h3>
                <Show when=move || is_owner.get()>
                    <button
                        type="button"
                        class="text-text-tertiary hover:text-text-primary text-base leading-none w-5 h-5 flex items-center justify-center rounded hover:bg-surface-sunken"
                        title=move || t_string!(i18n, project_room.add_member).to_string()
                        on:click=move |_| picking.update(|v| *v = !*v)
                    >
                        "+"
                    </button>
                </Show>
            </div>

            <Show when=move || picking.get()>
                <div class="mb-2 rounded-md border border-border bg-surface-sunken max-h-40 overflow-y-auto">
                    {move || {
                        let members = members.get_value();
                        // `selectable`, not `all`: a deactivated principal is
                        // still a name this directory must be able to render
                        // (an existing roster row, a historical bubble), but
                        // offering them here produces a write the server
                        // declines — and that refusal reads as a broken roster
                        // rather than as the state of that person.
                        let list: Vec<(String, String)> = dir
                            .selectable()
                            .into_iter()
                            .filter(|(uid, _)| !members.contains(uid))
                            .collect();
                        if list.is_empty() {
                            view! {
                                <p class="px-3 py-2 text-xs text-text-tertiary">
                                    {t!(i18n, project_room.no_users_to_add)}
                                </p>
                            }
                                .into_any()
                        } else {
                            list.into_iter().map(|(uid, name)| {
                                let uid = StoredValue::new(uid);
                                view! {
                                    <button
                                        type="button"
                                        class="w-full text-left px-3 py-1.5 text-sm hover:bg-surface-raised"
                                        on:click=move |_| {
                                            let uid = uid.get_value();
                                            let project_id = project_id.get_value();
                                            picking.set(false);
                                            spawn_local(async move {
                                                let result = ProjectsApi::member_add(&dash, &project_id, &uid)
                                                    .await
                                                    .map(|_| ());
                                                on_done.run(result);
                                            });
                                        }
                                    >
                                        {name}
                                    </button>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
            </Show>

            <ul class="space-y-1">
                {project.member_ids.into_iter().map(|uid| {
                    let is_the_owner = owner_id.as_deref() == Some(uid.as_str());
                    let uid = StoredValue::new(uid);
                    let display = move || dir.display_name(&uid.get_value());
                    view! {
                        <li class="flex items-center justify-between px-3 py-1.5 rounded-md bg-surface-sunken">
                            <span class="text-sm text-text-primary">{display}</span>
                            <span class="flex items-center gap-2">
                                <Show when=move || is_the_owner>
                                    <span class="text-[10px] uppercase tracking-wide text-text-tertiary">{t!(i18n, project_room.owner)}</span>
                                </Show>
                                <Show when=move || is_owner.get() && !is_the_owner>
                                    <button
                                        type="button"
                                        class="text-text-tertiary hover:text-danger text-xs px-1"
                                        title=move || t_string!(i18n, project_room.remove_member).to_string()
                                        on:click=move |_| {
                                            let uid = uid.get_value();
                                            let project_id = project_id.get_value();
                                            spawn_local(async move {
                                                let result = ProjectsApi::member_remove(&dash, &project_id, &uid)
                                                    .await
                                                    .map(|_| ());
                                                on_done.run(result);
                                            });
                                        }
                                    >
                                        {t!(i18n, project_room.remove)}
                                    </button>
                                </Show>
                            </span>
                        </li>
                    }
                }).collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn WorkspaceSection(
    project: ProjectInfo,
    is_owner: Memo<bool>,
    dash: DashboardState,
    on_done: OnDone,
) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(project.id.clone());
    let current = project.workspace_path.clone();
    let has_current = current.is_some();
    let browser_open = RwSignal::new(false);

    let on_pick = Callback::new(move |path: String| {
        let id = id.get_value();
        spawn_local(async move {
            let result = ProjectsApi::bind_workspace(&dash, &id, Some(&path))
                .await
                .map(|_| ());
            on_done.run(result);
        });
    });

    view! {
        <section>
            <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-2">{t!(i18n, project_room.workspace_binding)}</h3>
            <p class="text-sm text-text-primary mb-2">
                {move || {
                    current
                        .clone()
                        .unwrap_or_else(|| {
                            t_string!(i18n, project_room.workspace_unbound).to_string()
                        })
                }}
            </p>
            <Show when=move || is_owner.get()>
                <div class="flex items-center gap-2">
                    <button
                        type="button"
                        class="px-3 py-1.5 rounded-md text-sm bg-primary/15 text-primary hover:bg-primary/25"
                        on:click=move |_| browser_open.set(true)
                    >
                        {move || {
                            if has_current {
                                t_string!(i18n, project_room.change_folder).to_string()
                            } else {
                                t_string!(i18n, project_room.bind_folder).to_string()
                            }
                        }}
                    </button>
                    <Show when=move || has_current>
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded-md text-sm text-text-tertiary hover:text-danger"
                            on:click=move |_| {
                                let id = id.get_value();
                                spawn_local(async move {
                                    let result = ProjectsApi::bind_workspace(&dash, &id, None).await.map(|_| ());
                                    on_done.run(result);
                                });
                            }
                        >
                            {t!(i18n, project_room.unbind)}
                        </button>
                    </Show>
                </div>
                <DirectoryBrowser
                    open=browser_open
                    on_pick=on_pick
                    title=t_string!(i18n, project_room.browser_title).to_string()
                    confirm_label=t_string!(i18n, project_room.browser_confirm).to_string()
                />
            </Show>
        </section>
    }
}

#[component]
fn ArchiveSection(
    project: ProjectInfo,
    is_owner: Memo<bool>,
    dash: DashboardState,
    on_done: OnDone,
) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(project.id.clone());
    let confirming = RwSignal::new(false);
    view! {
        <Show when=move || is_owner.get()>
            <section class="border-t border-border-subtle pt-6">
                <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-2">{t!(i18n, project_room.danger_zone)}</h3>
                <Show
                    when=move || confirming.get()
                    fallback=move || view! {
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded-md text-sm text-danger hover:bg-danger/10"
                            on:click=move |_| confirming.set(true)
                        >
                            {t!(i18n, project_room.archive)}
                        </button>
                    }
                >
                    <div class="flex items-center gap-2">
                        <span class="text-sm text-text-secondary">
                            {t!(i18n, project_room.archive_confirm)}
                        </span>
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded-md text-sm bg-danger text-white hover:bg-danger/90"
                            on:click=move |_| {
                                let id = id.get_value();
                                spawn_local(async move {
                                    let result = ProjectsApi::archive(&dash, &id).await.map(|_| ());
                                    on_done.run(result);
                                });
                            }
                        >
                            {t!(i18n, common.confirm)}
                        </button>
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded-md text-sm text-text-tertiary"
                            on:click=move |_| confirming.set(false)
                        >
                            {t!(i18n, common.cancel)}
                        </button>
                    </div>
                </Show>
            </section>
        </Show>
    }
}
