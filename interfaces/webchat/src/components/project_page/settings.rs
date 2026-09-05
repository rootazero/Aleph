//! Project room settings tab (P2 Task 8, spec §6.4) — roster, workspace
//! binding, rename, archive.
//!
//! ## What `is_owner` may and may not do here
//!
//! The `is_owner` threaded through these sections is a CLIENT-side
//! approximation — `my_user_id == project.owner_user_id` — and it is strictly
//! NARROWER than the rule the server enforces. `projects::authz::is_owner`
//! also admits an org admin, and resolves a NULL `owner_user_id` through
//! `gateway::visibility::owner_or_legacy`. Neither widening is available on
//! this side: no `OWNER_USER_ID` is exported from `shared/protocol` at all,
//! and re-deriving the legacy rule here would be a third spelling of
//! something `visibility.rs` forbids duplicating by name.
//!
//! So this predicate may only ever *soften* a control, never remove one. A
//! section that is simply absent reads as "this room cannot be archived"
//! rather than "you may not archive it" — and an org admin, whom the server
//! would have accepted, never learns the feature exists. In `WorkspaceSection`
//! and `ArchiveSection` every ownership-gated control therefore renders, is
//! `disabled` when this client cannot prove ownership, and carries the reason
//! as its tooltip; the enabled path still calls and still reports whatever
//! came back through `admin_refusal::settings_write_error` (a rename can be
//! refused for reasons this predicate knows nothing about — ownership
//! transferred while the tab was open, or `require_directory_choice` declining
//! a chat-tier device). The sibling `channel_bindings` module states the same
//! rule for its own, admin-gated, pair of verbs.
//!
//! **Two sites are not yet converted** and still vanish for a non-owner, for
//! the same reason and with the same defect: `RosterSection`'s add-member `+`
//! button and its per-row Remove. They go with the server-derived
//! `manageable: bool` task named below — this is the honest state of the
//! sweep, not a claim that it is finished, and
//! `tests::the_roster_is_the_named_exception` keeps this paragraph and that
//! code in step in both directions.
//!
//! The honest predicate is that server-derived `manageable: bool` on the
//! project row, computed by the same derivation that enforces it. Until it
//! exists, a non-owner gets a read-only view of the workspace and archive
//! controls — not a room that appears to have no settings — and, at those two
//! roster sites, still no view at all.
//!
//! 工作区浏览 / 记忆浏览 are still P3 placeholders rendered by `ProjectRoomPage`
//! (the parent); 看板 is live in `project_page::kanban`. This file is the 设置
//! tab body only.
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

use super::channel_bindings::ChannelBindingsSection;
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
    //
    // Narrower than the server's rule — see the module doc. It may disable a
    // control; it may not decide whether one exists.
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
            // Takes neither `is_owner` nor `on_done`, and both omissions are
            // deliberate. `projects.channel.bind`/`.unbind` are ADMIN-gated,
            // not owner-gated, so `is_owner` would be wrong in both directions
            // — and this section's receipts have to say more than "it worked"
            // (what happened to the conversation's existing transcript is a
            // three-valued answer), which the shared `Result<(), String>`
            // callback cannot carry. See that module's doc.
            <ChannelBindingsSection project_id=project.id.clone() dash=dash />
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
    // A row with no stamped owner badges nobody, and an absent badge reads as a
    // fact about the roster ("this room has no owner") rather than as this
    // view's inability to answer. The server resolves such a row through
    // `visibility::owner_or_legacy`; that constant never crosses the wire, so
    // the only honest thing this side can do is say it does not know.
    let owner_unrecorded = owner_id.is_none();
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

            <Show when=move || owner_unrecorded>
                <p class="mb-2 text-xs text-text-tertiary">
                    {t!(i18n, project_room.owner_unrecorded)}
                </p>
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

    // What a control this client left off must say. Empty for an owner so the
    // tooltip does not follow a control that works; `Copy` (it captures only
    // `Memo` + `I18nContext`) so both buttons can carry the same closure.
    let owner_only_hint = move || {
        if is_owner.get() {
            String::new()
        } else {
            t_string!(i18n, project_room.owner_only).to_string()
        }
    };

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
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-md text-sm bg-primary/15 text-primary hover:bg-primary/25 disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=move || !is_owner.get()
                    title=owner_only_hint
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
                        class="px-3 py-1.5 rounded-md text-sm text-text-tertiary hover:text-danger disabled:opacity-50 disabled:cursor-not-allowed"
                        disabled=move || !is_owner.get()
                        title=owner_only_hint
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
            // Mounted unconditionally with the controls: it is a modal that
            // renders nothing until `browser_open` is set, and the only thing
            // that sets it is the disabled-when-not-owner button above.
            <DirectoryBrowser
                open=browser_open
                on_pick=on_pick
                title=t_string!(i18n, project_room.browser_title).to_string()
                confirm_label=t_string!(i18n, project_room.browser_confirm).to_string()
            />
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
    // See `WorkspaceSection` for why this exists and why it is empty for an
    // owner.
    let owner_only_hint = move || {
        if is_owner.get() {
            String::new()
        } else {
            t_string!(i18n, project_room.owner_only).to_string()
        }
    };
    view! {
        <section class="border-t border-border-subtle pt-6">
            <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-2">{t!(i18n, project_room.danger_zone)}</h3>
            <Show
                when=move || confirming.get()
                fallback=move || view! {
                    <button
                        type="button"
                        class="px-3 py-1.5 rounded-md text-sm text-danger hover:bg-danger/10 disabled:opacity-50 disabled:cursor-not-allowed"
                        disabled=move || !is_owner.get()
                        title=owner_only_hint
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
                    // Gated a second time on purpose: `is_owner` is reactive
                    // and a refresh can flip it while this confirmation is
                    // open (ownership handed over in another tab), which would
                    // otherwise leave a live Confirm behind a trigger that has
                    // already gone dead.
                    <button
                        type="button"
                        class="px-3 py-1.5 rounded-md text-sm bg-danger text-white hover:bg-danger/90 disabled:opacity-50 disabled:cursor-not-allowed"
                        disabled=move || !is_owner.get()
                        title=owner_only_hint
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
                    // Cancel only closes the confirmation. It is nobody's
                    // permission, so it is not gated.
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
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    const EN: &str = include_str!("../../../locales/en.json");
    const ZH: &str = include_str!("../../../locales/zh.json");

    /// Only the production code of this file.
    ///
    /// The RED fixtures below are copies of shapes this file has shipped, so an
    /// unscoped scan would count them as production sites and every predicate
    /// here would report the defect it exists to forbid.
    ///
    /// Through the crate's single answer rather than a local
    /// `split("#[cfg(test)]")` — that cut stops at the first *attribute*, not
    /// at the test module, and `i18n_census`'s own guard
    /// (`no_guard_in_this_crate_hand_rolls_the_cfg_test_cut`) refuses a second
    /// hand-rolled one. Whole-line comments are dropped with it, which is why
    /// the module-doc assertion below reads the raw source instead.
    fn production_source(src: &str) -> String {
        crate::i18n_census::production_lines(src)
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The body of one `#[component]` in this file.
    ///
    /// Panics rather than returning an empty body when the header is gone: a
    /// renamed component must fail loudly, not certify every predicate below by
    /// leaving them nothing to look at.
    fn body_of(src: &str, header: &str) -> String {
        let src = production_source(src);
        let Some((_, after)) = src.split_once(header) else {
            panic!("{header} is no longer in this file");
        };
        after
            .split("\n#[component]")
            .next()
            .unwrap_or(after)
            .to_string()
    }

    /// A permission gate that makes the section VANISH — the defect.
    ///
    /// `<Show when=..>` with no `fallback` renders nothing at all, so an
    /// affected person reads "this room cannot be archived" where the truth is
    /// "you may not archive it", and an org admin — whom the server's
    /// `authz::is_owner` admits and this client's narrower `is_owner` does not
    /// — never learns the feature exists.
    fn hides_behind_ownership(body: &str) -> bool {
        body.contains("<Show when=move || is_owner.get()>")
    }

    const DISABLED: &str = "disabled=move || !is_owner.get()";
    const HINT: &str = "title=owner_only_hint";

    /// Controls this client leaves off because it cannot prove ownership.
    fn disabled_controls(body: &str) -> usize {
        body.matches(DISABLED).count()
    }

    /// Every ownership gate that is not paired with the tooltip explaining it,
    /// and every such tooltip that gates nothing.
    ///
    /// Deliberately not two counts: a control that gains `title=owner_only_hint`
    /// without `disabled`, plus another that gains `disabled` without the
    /// tooltip, keep the totals equal while both controls lie (criterion #2 —
    /// N categories fanned into one value). So this pairs them line by line.
    fn unpaired_ownership_gates(body: &str) -> Vec<String> {
        /// The two attributes sit on adjacent lines today; allow a little slack
        /// for a formatter, not enough for a different control.
        const WINDOW: usize = 2;
        let lines: Vec<&str> = body.lines().collect();
        let near = |i: usize, needle: &str| {
            let lo = i.saturating_sub(WINDOW);
            let hi = (i + WINDOW + 1).min(lines.len());
            lines[lo..hi].iter().any(|line| line.contains(needle))
        };
        let mut unpaired = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.contains(DISABLED) && !near(i, HINT) {
                unpaired.push(format!("line {i}: `disabled` with no `{HINT}`"));
            }
            if line.contains(HINT) && !near(i, DISABLED) {
                unpaired.push(format!("line {i}: `{HINT}` gating nothing"));
            }
        }
        unpaired
    }

    /// Buttons whose block does not close the confirmation — i.e. the ones that
    /// mutate. Cancel only closes it and is nobody's permission, so it is
    /// deliberately not gated; deriving the expectation this way keeps a THIRD
    /// mutating control from arriving under a hard-coded count (criterion #5).
    fn mutating_buttons(body: &str) -> usize {
        body.split("<button")
            .skip(1)
            .filter(|block| !block.contains("confirming.set(false)"))
            .count()
    }

    fn room_copy(src: &str, key: &str) -> String {
        let all: serde_json::Value = serde_json::from_str(src).expect("locale file is JSON");
        all["project_room"][key]
            .as_str()
            .unwrap_or_else(|| panic!("locale file is missing project_room.{key}"))
            .to_string()
    }

    #[test]
    fn the_workspace_controls_render_disabled_rather_than_vanishing() {
        let body = body_of(include_str!("settings.rs"), "fn WorkspaceSection(");
        assert!(
            !hides_behind_ownership(&body),
            "the workspace controls are hidden from a non-owner again — a missing \
             control reads as a missing capability"
        );
        let buttons = body.matches("<button").count();
        assert!(buttons > 0, "the workspace section has no controls left");
        assert_eq!(
            disabled_controls(&body),
            buttons,
            "a workspace control is rendered without the ownership `disabled` \
             binding — it looks available to someone the server will refuse"
        );
    }

    #[test]
    fn the_archive_section_and_its_danger_heading_always_render() {
        let body = body_of(include_str!("settings.rs"), "fn ArchiveSection(");
        assert!(
            !hides_behind_ownership(&body),
            "the whole archive section (including its danger-zone heading) is \
             hidden from a non-owner again"
        );
        assert!(
            body.contains("project_room.danger_zone"),
            "the danger-zone heading left this section"
        );
        // Open-the-confirmation and confirm-it mutate; Cancel only closes the
        // confirmation and is nobody's permission.
        let mutating = mutating_buttons(&body);
        assert!(
            mutating > 0,
            "the archive section has no mutating control left"
        );
        assert_eq!(
            disabled_controls(&body),
            mutating,
            "a mutating archive control is rendered without the ownership \
             `disabled` binding — it looks available to someone the server \
             will refuse"
        );
    }

    #[test]
    fn every_control_this_client_leaves_off_says_why() {
        let src = production_source(include_str!("settings.rs"));
        assert!(
            disabled_controls(&src) > 0,
            "no control is gated by ownership any more"
        );
        let unpaired = unpaired_ownership_gates(&src);
        assert!(
            unpaired.is_empty(),
            "a control is disabled with no `title=owner_only_hint`, or explains \
             itself while gating nothing — the client is pretending the \
             capability does not exist instead of reporting what the server \
             would say: {unpaired:?}"
        );
    }

    #[test]
    fn the_roster_says_so_when_no_owner_is_recorded() {
        let body = body_of(include_str!("settings.rs"), "fn RosterSection(");
        assert!(
            body.contains("owner_id.is_none()") && body.contains("project_room.owner_unrecorded"),
            "a NULL-owner room badges nobody and says nothing — absence of the \
             badge reads as a fact about the roster rather than as this view's \
             inability to resolve the server's `owner_or_legacy`"
        );
    }

    #[test]
    fn the_module_doc_no_longer_promises_to_hide_the_controls() {
        // Raw, not `production_source`: that drops whole-line comments, and the
        // module doc is nothing but those. Cutting at the first `use` keeps the
        // test module (and its own prose) out without going near `#[cfg(test)]`.
        let src = include_str!("settings.rs");
        let doc = src.split("\nuse ").next().unwrap_or(src);
        for stale in [
            "hiding the controls",
            "mirrors the server's own `require_owner`",
        ] {
            assert!(
                !doc.contains(stale),
                "the module doc still claims {stale:?} — the comment is the \
                 expensive half of criterion #1, and this file no longer does that"
            );
        }
    }

    /// The module doc claims every ownership-gated control renders. Two do
    /// not. This pins the exception in BOTH directions so the claim and the
    /// code cannot drift apart: convert the roster and this test demands the
    /// paragraph go, delete the paragraph and it demands the roster be
    /// converted. A new fallback-less gate in Workspace/Archive is still
    /// caught by the two assertions above.
    #[test]
    fn the_roster_is_the_named_exception() {
        let src = include_str!("settings.rs");
        let doc = src.split("\nuse ").next().unwrap_or(src);
        assert!(
            hides_behind_ownership(&body_of(src, "fn RosterSection(")),
            "RosterSection no longer hides its controls — drop the exception \
             paragraph from the module doc in this same edit, or the doc keeps \
             naming a defect that is gone"
        );
        assert!(
            doc.contains("RosterSection"),
            "the module doc says every ownership-gated control renders and no \
             longer names the two roster sites that still vanish — a swept \
             invariant that is not swept is criterion #1 at its most expensive, \
             because it will be cited as evidence the sweep is done"
        );
    }

    #[test]
    fn both_new_sentences_exist_in_both_languages_and_say_different_things() {
        for (lang, src) in [("en", EN), ("zh", ZH)] {
            let mut seen = BTreeSet::new();
            for key in ["owner_only", "owner_unrecorded"] {
                let text = room_copy(src, key);
                assert!(
                    !text.trim().is_empty(),
                    "{lang}: project_room.{key} is empty"
                );
                assert!(
                    seen.insert(text.clone()),
                    "{lang}: project_room.{key} reuses the other sentence — \
                     'you may not' and 'nobody is recorded' are two answers"
                );
            }
        }
    }

    /// RED proof — the shape this file shipped: the section exists only for an
    /// owner, so nothing is disabled because nothing is rendered.
    #[test]
    fn the_gate_check_rejects_the_fallback_less_show_this_file_shipped() {
        // Brace-balanced on purpose: `i18n_census::end_of_gated_item` counts
        // braces line by line and does not carry a string literal across
        // lines, so a fixture that closes more than it opens would end the
        // enclosing `mod tests` early and hand the rest of it to the scanners
        // above as production code.
        let before = "
fn ArchiveSection(
    view! {
        <Show when=move || is_owner.get()>
            <section class=\"border-t border-border-subtle pt-6\">
                <h3>danger zone</h3>
                <button on:click=move |_| confirming.set(true)>archive</button>
            </section>
        </Show>
    }
";
        let body = body_of(before, "fn ArchiveSection(");
        assert!(hides_behind_ownership(&body));
        assert_eq!(disabled_controls(&body), 0);
    }

    /// RED proof for the other half: dropping the `<Show>` alone is not the
    /// fix. A control that renders enabled for someone the server will refuse
    /// is a different lie, and the mutation "delete the `disabled` binding"
    /// must land here.
    #[test]
    fn the_disabled_check_rejects_a_control_that_only_stopped_hiding() {
        // Brace-balanced, for the reason given on the fixture above.
        let before = "
fn WorkspaceSection(
    view! {
        <section>
            <div class=\"flex items-center gap-2\">
                <button on:click=move |_| browser_open.set(true)>bind</button>
            </div>
        </section>
    }
";
        let body = body_of(before, "fn WorkspaceSection(");
        assert!(!hides_behind_ownership(&body));
        assert_ne!(disabled_controls(&body), body.matches("<button").count());
    }
}
