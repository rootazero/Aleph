//! Project room Kanban tab (P3, spec §6.4) — a read-only overview of the work
//! that lives in this room: its teams' task boards, its goals, its loops.
//!
//! ## What this tab is, and what it deliberately is not
//! It summarises; it does not edit. Each room team renders the canonical
//! column chips built from [`board_columns`] — the same table the full board
//! and the team-chat task strip read — but not the drag-and-drop board
//! itself. Moving a task stays in the Teams view, which owns that path
//! (`KanbanView` + `lifecycle::apply_move`, including the destructive-drop
//! confirm). Forking it here would make two surfaces answer "how do you move
//! a task", and the second answer is the one that drifts.
//!
//! Reusing `board_columns` rather than re-listing statuses matters for the
//! same reason: a hand-written column list here would silently omit whatever
//! status the server grows next, and a missing column renders as "those tasks
//! do not exist" rather than as a gap.
//!
//! ## Membership is decided by the stamp, not by the fetch
//! `teams.list` returns every team the caller can see, across all rooms; the
//! room filter is client-side on `scope_id`, via
//! [`aleph_protocol::scope::belongs_to_project`]. Goals and loops go the other
//! way — the server filters them, gated through the roster — because those
//! RPCs accept a `scope_id` and `teams.list` does not. Both directions agree
//! on one spelling of a project scope id because both take it from the
//! protocol crate.
//!
//! An unstamped team (`scope_id: None`) belongs to no room. That is the
//! fail-closed direction: teams created before scope stamping carry `None`,
//! and admitting them here would show one room a team it does not own.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::kanban::{GoalRow, KanbanApi, LoopRow};
use crate::api::projects::ProjectInfo;
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamSummary, TeamsApi};
use crate::components::team_chat_entry::enter_team_chat;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, Locale};
use crate::platform::wide::views::chat::state::ChatState;
use crate::platform::wide::views::teams::components::board_columns::{
    column_label, count_for_column, BOARD_COLUMNS,
};
use leptos_i18n::I18nContext;

/// What to show when one of this tab's reads failed.
///
/// Every RPC this tab calls belongs to an admin-gated family, so a plain
/// member's refusal arrives as a raw protocol string. Routing it through the
/// shared classifier turns that into "you cannot read this" rather than
/// leaking the wire text; on a surface that is never refused the wrapper is a
/// byte-for-byte no-op, which is why the rule it satisfies has no allowlist.
fn load_failure(i18n: I18nContext<Locale>, err: &str) -> String {
    crate::components::admin_refusal::settings_load_error(i18n, err, |e| e.to_string())
}

/// The room's teams, out of everything `teams.list` returned.
///
/// Pure and separate from the fetch so the membership rule is testable
/// without a Leptos owner or a live gateway — the rule is the part that can be
/// wrong in a way nobody sees, because a filter matching nothing renders as an
/// empty room rather than as an error.
#[must_use]
pub(crate) fn teams_in_room(all: &[TeamSummary], project_id: &str) -> Vec<TeamSummary> {
    all.iter()
        .filter(|t| aleph_protocol::scope::belongs_to_project(t.scope_id.as_deref(), project_id))
        .cloned()
        .collect()
}

#[component]
#[must_use]
pub fn KanbanTab(project: ProjectInfo) -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();

    let teams: RwSignal<Vec<TeamSummary>> = RwSignal::new(Vec::new());
    let goals: RwSignal<Vec<GoalRow>> = RwSignal::new(Vec::new());
    let loops: RwSignal<Vec<LoopRow>> = RwSignal::new(Vec::new());
    // `Some(msg)` only when a call actually failed. A load that returns nothing
    // is not an error, and rendering it as one would tell the user the gateway
    // is broken when the room is merely empty.
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    let project_id = StoredValue::new(project.id.clone());

    let refresh = move || {
        let pid = project_id.get_value();
        spawn_local(async move {
            match TeamsApi::list(&dash).await {
                Ok(all) => teams.set(teams_in_room(&all, &pid)),
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
            match KanbanApi::goals_for_project(&dash, &pid).await {
                Ok(rows) => goals.set(rows),
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
            match KanbanApi::loops_for_project(&dash, &pid).await {
                Ok(rows) => loops.set(rows),
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
        });
    };

    // Load on mount, and again whenever the gateway reconnects: a tab that
    // mounted before the socket was authorized would otherwise sit empty
    // forever, which is indistinguishable from an empty room.
    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        }
    });

    view! {
        <div class="flex-1 overflow-auto px-6 py-4 space-y-6">
            <Show when=move || error.get().is_some()>
                <div class="rounded-md bg-danger-subtle px-3 py-2 text-xs text-danger">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <section class="space-y-3">
                <h3 class="text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                    {t!(i18n, project_room.kanban_teams)}
                </h3>
                <Show
                    when=move || !teams.get().is_empty()
                    fallback=move || view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, project_room.kanban_empty)}
                        </p>
                    }
                >
                    <div class="space-y-3">
                        <For each=move || teams.get() key=|t: &TeamSummary| t.id.clone() let:team>
                            <RoomTeamCard team=team />
                        </For>
                    </div>
                </Show>
            </section>

            <section class="space-y-2">
                <h3 class="text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                    {t!(i18n, project_room.kanban_goals)}
                </h3>
                <Show
                    when=move || !goals.get().is_empty()
                    fallback=move || view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, project_room.kanban_no_goals)}
                        </p>
                    }
                >
                    <ul class="space-y-1">
                        <For each=move || goals.get() key=|g: &GoalRow| g.session_id.clone() let:goal>
                            <li class="flex items-center gap-2 rounded-md bg-surface-raised px-3 py-2 text-sm">
                                <StatusChip status=goal.status.clone() />
                                <span class="truncate">{goal.objective.clone()}</span>
                            </li>
                        </For>
                    </ul>
                </Show>
            </section>

            <section class="space-y-2">
                <h3 class="text-xs font-semibold uppercase tracking-wide text-text-tertiary">
                    {t!(i18n, project_room.kanban_loops)}
                </h3>
                <Show
                    when=move || !loops.get().is_empty()
                    fallback=move || view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, project_room.kanban_no_loops)}
                        </p>
                    }
                >
                    <ul class="space-y-1">
                        <For each=move || loops.get() key=|l: &LoopRow| l.session_id.clone() let:lp>
                            <li class="flex items-center gap-2 rounded-md bg-surface-raised px-3 py-2 text-sm">
                                <StatusChip status=lp.status.clone() />
                                <span class="truncate">{lp.prompt.clone()}</span>
                            </li>
                        </For>
                    </ul>
                </Show>
            </section>
        </div>
    }
}

/// One room team: its name, its board's column chips, and a way into its
/// group chat.
///
/// Fetches its own tasks rather than having the parent fan out, so one team
/// whose task list fails does not blank every other team's chips.
#[component]
fn RoomTeamCard(team: TeamSummary) -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let team_id = StoredValue::new(team.id.clone());

    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let id = team_id.get_value();
        spawn_local(async move {
            if let Ok(list) = TeamsApi::list_tasks(&dash, &id, TaskFilter::default()).await {
                tasks.set(list);
            }
        });
    });

    let open_chat = move |_| {
        let id = team_id.get_value();
        spawn_local(async move {
            // `None` for the agent roster: this surface never loaded
            // `agents.list`, and a name falling back to its id is a cosmetic
            // loss where an extra round trip on every click is not.
            enter_team_chat(dash, chat, None, id).await;
        });
    };

    let name = team.name.clone();
    let subtitle = format!("{} · {}", team.status, team.member_count);

    view! {
        <div class="rounded-lg border border-border-subtle bg-surface-raised p-3 space-y-2">
            <div class="flex items-center justify-between gap-2">
                <div class="min-w-0">
                    <div class="truncate text-sm font-medium">{name}</div>
                    <div class="text-xs text-text-tertiary">{subtitle}</div>
                </div>
                <button
                    class="shrink-0 rounded-md border border-border-subtle px-2 py-1 text-xs hover:bg-surface-hover"
                    on:click=open_chat
                >
                    {t!(i18n, project_room.kanban_open_chat)}
                </button>
            </div>
            <div class="flex flex-wrap gap-1.5">
                {BOARD_COLUMNS.iter().map(|col| {
                    let status = col.status;
                    let title = column_label(i18n, status);
                    let count = Signal::derive(move || count_for_column(&tasks.get(), status));
                    view! {
                        <span
                            class="rounded-full bg-surface-sunken px-2 py-0.5 text-[11px] text-text-secondary"
                            class:opacity-40=move || count.get() == 0
                        >
                            {format!("{title} ")}{move || count.get()}
                        </span>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

/// A small status pill shared by the goal and loop rows.
#[component]
fn StatusChip(status: String) -> impl IntoView {
    view! {
        <span class="shrink-0 rounded-full bg-surface-sunken px-2 py-0.5 text-[11px] text-text-secondary">
            {status}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(id: &str, scope: Option<&str>) -> TeamSummary {
        TeamSummary {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            leader_id: "main".into(),
            status: "active".into(),
            member_count: 0,
            task_count: 0,
            created_at: 0,
            disbanded_at: None,
            members_preview: Vec::new(),
            last_message: None,
            last_message_at: None,
            scope_id: scope.map(str::to_string),
        }
    }

    /// The three ways a team can fail to belong to this room, each of which a
    /// naive equality or truthiness check gets wrong in a different direction.
    #[test]
    fn only_this_rooms_stamped_teams_are_shown() {
        let all = vec![
            team("mine", Some("project:p-a")),
            team("other_room", Some("project:p-b")),
            team("personal", Some("personal:u-alice")),
            team("unstamped", None),
        ];
        let ids: Vec<String> = teams_in_room(&all, "p-a")
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["mine".to_string()]);
    }

    /// A room with no teams shows none, never every team the caller can see —
    /// the failure mode of a filter written the wrong way round.
    #[test]
    fn a_room_with_no_teams_shows_none_of_the_others() {
        let all = vec![
            team("other_room", Some("project:p-b")),
            team("unstamped", None),
        ];
        assert!(teams_in_room(&all, "p-a").is_empty());
    }

    /// Driven by the shared spelling, so a room id that happens to be a prefix
    /// of another must not match it.
    #[test]
    fn a_prefix_of_another_room_id_does_not_match() {
        let all = vec![team("longer", Some("project:p-abc"))];
        assert!(teams_in_room(&all, "p-a").is_empty());
    }
}
