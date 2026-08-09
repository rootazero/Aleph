//! Workspace roster — `/settings/workspaces`.
//!
//! The Panel half of `workspace.{list,get,create,update,archive,unarchive}`,
//! which until 2026-08-09 had exactly one client and it was the CLI. Everything
//! `aleph workspace …` can do is here, including the verb that arrived with
//! this page: **unarchive**, the reason `archive` stopped being a one-way door.
//!
//! # A refused read is not an empty roster
//!
//! The whole `workspace.` family is admin-gated server-side
//! (`gateway::method_admin`), so a member reaches this page and every call on
//! it comes back refused. That is the interesting case, not the edge case:
//! "you have no workspaces" and "you may not ask" render identically unless
//! something insists on the difference, and the confident false statement is
//! the expensive one. Reads go through
//! [`admin_refusal::settings_load_error`] and writes through
//! [`admin_refusal::labeled`]; a transport failure or a malformed response
//! keeps its own words, because neither is a permission verdict.
//!
//! This page is **not** hidden from members. `components::admin_refusal`'s own
//! doc explains why — a client-side role predicate is the same gate under a new
//! name, and the Panel deliberately deleted the one it had. Render, call,
//! report what the server said.

use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceDetail, WorkspaceRow, WorkspaceUpdateParams,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::workspace::WorkspaceApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// What the right-hand pane is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pane {
    /// Nothing picked yet.
    Empty,
    /// The create form.
    New,
    /// An existing workspace, addressed by id.
    ///
    /// By id rather than by list index: the list reloads on every write and
    /// after an `include_archived` toggle, and an index into a list that has
    /// been refetched underneath the editor is how a save lands on the wrong
    /// row.
    Edit(String),
}

#[component]
#[must_use]
pub fn WorkspacesView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let rows = RwSignal::new(Vec::<WorkspaceRow>::new());
    let include_archived = RwSignal::new(false);
    let pane = RwSignal::new(Pane::Empty);
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(Option::<String>::None);
    let action_error = RwSignal::new(Option::<String>::None);

    // Reload whenever the view widens or narrows, and once the socket is up.
    // Tracks `include_archived` and `is_connected`, so neither the toggle nor
    // the connection needs a handler of its own.
    Effect::new(move || {
        if !state.is_connected.get() {
            // Not asked yet. Deliberately NOT an error and deliberately not an
            // empty roster: a page that renders "no workspaces" before it has
            // managed to ask is the same defect as one that renders it after
            // being refused. Tracking the signal is also what makes the first
            // real load happen — a bare `spawn_local` on mount fires against a
            // socket that is usually still connecting, fails with "Not
            // connected", and never retries.
            return;
        }
        let with_archived = include_archived.get();
        loading.set(true);
        spawn_local(async move {
            match WorkspaceApi::list(&state, with_archived).await {
                Ok(list) => {
                    rows.set(list);
                    load_error.set(None);
                }
                Err(e) => {
                    // The roster is NOT cleared on failure. A refusal says
                    // nothing about what is there, and blanking the list would
                    // be this page answering a question it was denied.
                    load_error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                        format!("Failed to load workspaces: {e}")
                    })));
                }
            }
            loading.set(false);
        });
    });

    let reload = move || {
        let with_archived = include_archived.get_untracked();
        spawn_local(async move {
            if let Ok(list) = WorkspaceApi::list(&state, with_archived).await {
                rows.set(list);
            }
        });
    };

    view! {
        <div class="flex flex-col h-full">
            <div class="p-6 border-b border-border aleph-content-top">
                <h1 class="text-2xl font-bold text-text-primary">
                    {t!(i18n, settings.workspaces.title)}
                </h1>
                <p class="mt-1 text-sm text-text-secondary">
                    {t!(i18n, settings.workspaces.description)}
                </p>
            </div>

            {move || {
                load_error
                    .get()
                    .map(|msg| {
                        view! {
                            <div class="mx-6 mt-4 px-4 py-3 rounded-lg bg-warning-subtle border border-warning text-sm text-text-primary">
                                {msg}
                            </div>
                        }
                    })
            }}

            <div class="flex-1 flex overflow-hidden">
                <RosterList
                    rows=rows
                    include_archived=include_archived
                    pane=pane
                    loading=loading
                    load_error=load_error
                />
                <WorkspaceEditor
                    pane=pane
                    action_error=action_error
                    on_changed=Callback::new(move |()| reload())
                />
            </div>
        </div>
    }
}

// ============================================================================
// Left pane — the roster
// ============================================================================

/// What the roster column says when it has no rows to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterEmptyState {
    /// A read is in flight.
    Loading,
    /// The read failed. Say nothing — the error banner above already explains,
    /// and anything here would be a second, contradictory answer.
    Silent,
    /// A read SUCCEEDED and came back with zero rows.
    NoWorkspaces,
}

/// The rule, as a function rather than three branches inside a `view!`.
///
/// Pulled out so it can be asserted. The bug it encodes was found by a human
/// looking at a real screen — a member saw the operator-privilege banner and
/// `暂无工作区` in the same viewport — and nothing in the component tree could
/// have failed for it, because "what does the empty column say" was a shape
/// buried in markup rather than a value anything could compare.
const fn roster_empty_state(loading: bool, failed: bool) -> RosterEmptyState {
    match (loading, failed) {
        // In flight wins even when a previous attempt failed: the stale error
        // is still on screen, but "loading" is the more recent truth.
        (true, _) => RosterEmptyState::Loading,
        (false, true) => RosterEmptyState::Silent,
        (false, false) => RosterEmptyState::NoWorkspaces,
    }
}

/// The roster column.
///
/// Takes `load_error` for one reason, and it is the whole reason: **"no
/// workspaces" is a claim about the world, and only a successful read may make
/// it.** A 2026-08-09 real-machine QA logged in as a member and got both
/// sentences at once — the banner saying "this page needs operator privileges"
/// and, right under it, `暂无工作区`. The banner was right and the list was
/// lying, in the same viewport, about the same failed call.
///
/// That is precisely the defect `components::admin_refusal` exists to prevent,
/// reappearing one level up: the module makes the *error* honest, and then a
/// second component renders an empty-state that never looked at the error. Any
/// list here that can fail owes the same gate.
#[component]
fn RosterList(
    rows: RwSignal<Vec<WorkspaceRow>>,
    include_archived: RwSignal<bool>,
    pane: RwSignal<Pane>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="w-80 border-r border-border flex flex-col">
            <div class="p-4 border-b border-border space-y-3">
                <button
                    on:click=move |_| pane.set(Pane::New)
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    {t!(i18n, settings.workspaces.new_workspace)}
                </button>
                <label class="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || include_archived.get()
                        on:change=move |_| include_archived.update(|v| *v = !*v)
                        class="accent-primary"
                    />
                    {t!(i18n, settings.workspaces.include_archived)}
                </label>
            </div>

            <div class="flex-1 overflow-y-auto p-4 space-y-2">
                {move || {
                    let list = rows.get();
                    if !list.is_empty() {
                        return list
                            .into_iter()
                            .map(|row| view! { <RosterRow row=row pane=pane /> })
                            .collect_view()
                            .into_any();
                    }
                    match roster_empty_state(loading.get(), load_error.get().is_some()) {
                        RosterEmptyState::Loading => {
                            view! {
                                <p class="text-sm text-text-tertiary">
                                    {t_string!(i18n, settings.workspaces.loading)}
                                </p>
                            }
                                .into_any()
                        }
                        RosterEmptyState::Silent => ().into_any(),
                        RosterEmptyState::NoWorkspaces => {
                            view! {
                                <p class="text-sm text-text-tertiary">
                                    {t_string!(i18n, settings.workspaces.no_workspaces)}
                                </p>
                            }
                                .into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn RosterRow(row: WorkspaceRow, pane: RwSignal<Pane>) -> impl IntoView {
    let i18n = use_i18n();
    let id = row.id.clone();
    let selected_id = id.clone();
    let is_selected = move || pane.get() == Pane::Edit(selected_id.clone());
    let archived = row.is_archived;
    let name = row.name.clone();
    let description = row.description.clone();

    view! {
        <button
            on:click=move |_| pane.set(Pane::Edit(id.clone()))
            class=move || {
                if is_selected() {
                    "w-full p-3 bg-primary-subtle border border-primary rounded-lg text-left transition-colors"
                } else {
                    "w-full p-3 bg-surface-sunken border border-border hover:border-border-strong rounded-lg text-left transition-colors"
                }
            }
        >
            <div class="flex items-center justify-between gap-2 mb-1">
                <span class="text-sm text-text-primary font-medium truncate">{name}</span>
                {archived
                    .then(|| {
                        view! {
                            <span class="shrink-0 px-2 py-0.5 rounded text-[10px] uppercase tracking-wide bg-surface-raised text-text-tertiary border border-border">
                                {t_string!(i18n, settings.workspaces.status_archived)}
                            </span>
                        }
                    })}
            </div>
            <div class="text-xs text-text-tertiary font-mono truncate">{row.id.clone()}</div>
            {description
                .map(|d| view! { <div class="mt-1 text-xs text-text-secondary truncate">{d}</div> })}
        </button>
    }
}

// ============================================================================
// Right pane — the editor
// ============================================================================

#[component]
fn WorkspaceEditor(
    pane: RwSignal<Pane>,
    action_error: RwSignal<Option<String>>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let detail = RwSignal::new(Option::<WorkspaceDetail>::None);
    let busy = RwSignal::new(false);

    let form_id = RwSignal::new(String::new());
    let form_name = RwSignal::new(String::new());
    let form_description = RwSignal::new(String::new());
    let form_icon = RwSignal::new(String::new());

    // Follow the selection. `Edit` refetches by id rather than reading the
    // list row: `profile`, `icon` and `last_active_at` are detail-only, and
    // they are the reason a detail view is worth a round trip at all.
    Effect::new(move || match pane.get() {
        Pane::Empty => {
            detail.set(None);
        }
        Pane::New => {
            detail.set(None);
            action_error.set(None);
            form_id.set(String::new());
            form_name.set(String::new());
            form_description.set(String::new());
            form_icon.set(String::new());
        }
        Pane::Edit(id) => {
            action_error.set(None);
            spawn_local(async move {
                match WorkspaceApi::get(&state, &id).await {
                    Ok(ws) => {
                        form_name.set(ws.name.clone());
                        form_description.set(ws.description.clone().unwrap_or_default());
                        form_icon.set(ws.icon.clone().unwrap_or_default());
                        detail.set(Some(ws));
                    }
                    Err(e) => {
                        detail.set(None);
                        action_error.set(Some(admin_refusal::labeled(
                            &e,
                            t_string!(i18n, settings.admin_refusal.manage_workspaces),
                        )));
                    }
                }
            });
        }
    });

    // One place that runs a mutation: set busy, call, adopt the row that came
    // back, report a failure through `admin_refusal::labeled`, refresh the
    // roster. Four buttons through one path so a refusal cannot be explained
    // four different ways.
    //
    // Every mutation resolves to the authoritative row and this sets `detail`
    // from it **directly**. The tempting alternative — re-`set` the selection
    // and let the effect above refetch — silently depends on whether Leptos
    // notifies a `set` to an equal value, which is not a thing to bet a
    // correctness property on: three of the four buttons act on the workspace
    // that is already selected, so if it dedupes, the pane keeps rendering the
    // pre-archive state (Active, with a Save button) over a row that is now
    // archived.
    let run = move |call: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkspaceDetail, String>>>,
    >| {
        busy.set(true);
        action_error.set(None);
        spawn_local(async move {
            match call.await {
                Ok(ws) => {
                    form_name.set(ws.name.clone());
                    form_description.set(ws.description.clone().unwrap_or_default());
                    form_icon.set(ws.icon.clone().unwrap_or_default());
                    pane.set(Pane::Edit(ws.id.clone()));
                    detail.set(Some(ws));
                    on_changed.run(());
                }
                Err(e) => {
                    action_error.set(Some(admin_refusal::labeled(
                        &e,
                        t_string!(i18n, settings.admin_refusal.manage_workspaces),
                    )));
                }
            }
            busy.set(false);
        });
    };

    let optional = |value: String| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    };

    let on_create = move |_| {
        let id = form_id.get().trim().to_string();
        if id.is_empty() {
            action_error.set(Some(
                t_string!(i18n, settings.workspaces.id_required).to_string(),
            ));
            return;
        }
        // The server defaults the display name to the id; sending the id
        // rather than an empty string keeps the two sides from disagreeing
        // about what that default is (`WorkspaceCreateParams::name`).
        let name = optional(form_name.get()).unwrap_or_else(|| id.clone());
        let params = WorkspaceCreateParams {
            id,
            name,
            description: optional(form_description.get()),
            icon: optional(form_icon.get()),
        };
        run(Box::pin(async move {
            WorkspaceApi::create(&state, params).await
        }));
    };

    let on_save = move |_| {
        let Some(id) = detail.get().map(|d| d.id) else {
            return;
        };
        let params = WorkspaceUpdateParams {
            id,
            name: optional(form_name.get()),
            description: optional(form_description.get()),
            icon: optional(form_icon.get()),
        };
        run(Box::pin(async move {
            WorkspaceApi::update(&state, params).await
        }));
    };

    // `archive` answers `{"ok": true}` and nothing else, so this reads the row
    // back explicitly. `workspace.get` reaches archived rows by design — the
    // pane has to be able to render the state it just produced, which is the
    // whole reason `get` was widened to `get_including_archived` server-side.
    let on_archive = move |_| {
        let Some(id) = detail.get().map(|d| d.id) else {
            return;
        };
        run(Box::pin(async move {
            WorkspaceApi::archive(&state, &id).await?;
            WorkspaceApi::get(&state, &id).await
        }));
    };

    let on_unarchive = move |_| {
        let Some(id) = detail.get().map(|d| d.id) else {
            return;
        };
        run(Box::pin(async move {
            WorkspaceApi::unarchive(&state, &id).await
        }));
    };

    let field = move |label: String,
                      value: RwSignal<String>,
                      editable: bool,
                      hint: Option<String>| {
        view! {
            <div>
                <label class="block text-sm font-medium text-text-secondary mb-2">{label}</label>
                <input
                    type="text"
                    prop:value=move || value.get()
                    prop:disabled=!editable
                    on:input=move |ev| value.set(event_target_value(&ev))
                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary disabled:opacity-60"
                />
                {hint.map(|h| view! { <p class="mt-1 text-xs text-text-tertiary">{h}</p> })}
            </div>
        }
    };

    view! {
        <div class="flex-1 overflow-y-auto p-6">
            {move || {
                match pane.get() {
                    Pane::Empty => {
                        view! {
                            <p class="text-sm text-text-tertiary">
                                {t_string!(i18n, settings.workspaces.select_or_add)}
                            </p>
                        }
                            .into_any()
                    }
                    Pane::New => {
                        view! {
                            <div class="max-w-xl space-y-5">
                                <h2 class="text-lg font-semibold text-text-primary">
                                    {t_string!(i18n, settings.workspaces.new_workspace)}
                                </h2>
                                {field(
                                    t_string!(i18n, settings.workspaces.id_label).to_string(),
                                    form_id,
                                    true,
                                    Some(t_string!(i18n, settings.workspaces.id_hint).to_string()),
                                )}
                                {field(
                                    t_string!(i18n, settings.workspaces.name_label).to_string(),
                                    form_name,
                                    true,
                                    Some(t_string!(i18n, settings.workspaces.name_hint).to_string()),
                                )}
                                {field(
                                    t_string!(i18n, settings.workspaces.description_label)
                                        .to_string(),
                                    form_description,
                                    true,
                                    None,
                                )}
                                {field(
                                    t_string!(i18n, settings.workspaces.icon_label).to_string(),
                                    form_icon,
                                    true,
                                    None,
                                )}
                                <ActionError action_error=action_error />
                                <button
                                    on:click=on_create
                                    prop:disabled=move || busy.get()
                                    class="px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors disabled:opacity-60"
                                >
                                    {t_string!(i18n, settings.workspaces.create)}
                                </button>
                            </div>
                        }
                            .into_any()
                    }
                    Pane::Edit(id) => {
                        let Some(ws) = detail.get() else {
                            return view! {
                                <div class="space-y-4">
                                    <p class="text-sm text-text-tertiary font-mono">{id}</p>
                                    <ActionError action_error=action_error />
                                </div>
                            }
                                .into_any();
                        };
                        let archived = ws.is_archived;
                        view! {
                            <div class="max-w-xl space-y-5">
                                <div class="flex items-center gap-3">
                                    <h2 class="text-lg font-semibold text-text-primary">
                                        {ws.name.clone()}
                                    </h2>
                                    <span class="px-2 py-0.5 rounded text-[10px] uppercase tracking-wide bg-surface-raised text-text-tertiary border border-border">
                                        {if archived {
                                            t_string!(i18n, settings.workspaces.status_archived)
                                        } else {
                                            t_string!(i18n, settings.workspaces.status_active)
                                        }}
                                    </span>
                                </div>

                                {archived
                                    .then(|| {
                                        view! {
                                            <p class="px-4 py-3 rounded-lg bg-surface-sunken border border-border text-sm text-text-secondary">
                                                {t_string!(i18n, settings.workspaces.archived_note)}
                                            </p>
                                        }
                                    })}

                                {field(
                                    t_string!(i18n, settings.workspaces.name_label).to_string(),
                                    form_name,
                                    !archived,
                                    None,
                                )}
                                {field(
                                    t_string!(i18n, settings.workspaces.description_label)
                                        .to_string(),
                                    form_description,
                                    !archived,
                                    None,
                                )}
                                {field(
                                    t_string!(i18n, settings.workspaces.icon_label).to_string(),
                                    form_icon,
                                    !archived,
                                    None,
                                )}

                                <dl class="grid grid-cols-2 gap-x-6 gap-y-2 text-sm">
                                    <dt class="text-text-tertiary">
                                        {t_string!(i18n, settings.workspaces.id_label)}
                                    </dt>
                                    <dd class="text-text-primary font-mono">{ws.id.clone()}</dd>
                                    <dt class="text-text-tertiary">
                                        {t_string!(i18n, settings.workspaces.profile_label)}
                                    </dt>
                                    <dd class="text-text-primary">{ws.profile.clone()}</dd>
                                    <dt class="text-text-tertiary">
                                        {t_string!(i18n, settings.workspaces.created_label)}
                                    </dt>
                                    <dd class="text-text-primary">{local_stamp(ws.created_at)}</dd>
                                    <dt class="text-text-tertiary">
                                        {t_string!(i18n, settings.workspaces.last_active_label)}
                                    </dt>
                                    <dd class="text-text-primary">
                                        {local_stamp(ws.last_active_at)}
                                    </dd>
                                </dl>

                                <ActionError action_error=action_error />

                                <div class="flex gap-3">
                                    {(!archived)
                                        .then(|| {
                                            view! {
                                                <button
                                                    on:click=on_save
                                                    prop:disabled=move || busy.get()
                                                    class="px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors disabled:opacity-60"
                                                >
                                                    {t_string!(i18n, settings.workspaces.save)}
                                                </button>
                                                <button
                                                    on:click=on_archive
                                                    prop:disabled=move || busy.get()
                                                    class="px-4 py-2 bg-surface-sunken border border-border hover:border-border-strong text-text-primary rounded-lg transition-colors disabled:opacity-60"
                                                >
                                                    {t_string!(i18n, settings.workspaces.archive)}
                                                </button>
                                            }
                                        })}
                                    {archived
                                        .then(|| {
                                            view! {
                                                <button
                                                    on:click=on_unarchive
                                                    prop:disabled=move || busy.get()
                                                    class="px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors disabled:opacity-60"
                                                >
                                                    {t_string!(i18n, settings.workspaces.unarchive)}
                                                </button>
                                            }
                                        })}
                                </div>
                            </div>
                        }
                            .into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn ActionError(action_error: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || {
            action_error
                .get()
                .map(|msg| {
                    view! {
                        <p class="px-4 py-3 rounded-lg bg-warning-subtle border border-warning text-sm text-text-primary">
                            {msg}
                        </p>
                    }
                })
        }}
    }
}

/// Render a UTC instant in the reader's own zone.
///
/// The wire is UTC — `WorkspaceDetail`'s doc says so — and a bare UTC clock in
/// a human-facing column does not read as "another timezone", it reads as the
/// wrong time. The CLI learned this on 2026-08-08 by shipping a `Created`
/// column that did.
fn local_stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused read must not produce "no workspaces".
    ///
    /// Found on a real screen, not here: a member opened this page and got the
    /// operator-privilege banner AND `暂无工作区`, one above the other, both
    /// describing the same failed call. The banner was honest; the list was
    /// answering a question it had been refused.
    ///
    /// This is `components::admin_refusal`'s defect one level up. That module
    /// makes the *error* honest and stops there — it cannot see a sibling
    /// component rendering an empty state that never looked at the error. Every
    /// list on a page whose reads can be refused owes this check.
    #[test]
    fn an_empty_roster_is_only_claimed_after_a_read_that_succeeded() {
        assert_eq!(
            roster_empty_state(false, true),
            RosterEmptyState::Silent,
            "a failed read must not be reported as an empty roster"
        );
        assert_eq!(
            roster_empty_state(false, false),
            RosterEmptyState::NoWorkspaces,
            "…but a SUCCESSFUL read that returned nothing must still say so — \
             otherwise this fix would have replaced a wrong answer with none"
        );
        // In flight wins over a stale error from the previous attempt.
        assert_eq!(roster_empty_state(true, true), RosterEmptyState::Loading);
        assert_eq!(roster_empty_state(true, false), RosterEmptyState::Loading);
    }
}
