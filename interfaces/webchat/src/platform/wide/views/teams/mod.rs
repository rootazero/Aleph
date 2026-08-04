//! Teams Tab (top-level panel mode).
//!
//! Houses five sub-views:
//! - Overview: existing collapsible team-cards list (migrated from /dashboard/teams)
//! - Kanban: drag-and-drop task board over `CoordTask` (one column per stored
//!   status; a drop routes through `lifecycle::resolve_move` onto the existing
//!   backend verbs, confirming destructive moves)
//! - Plan: read-only layered DAG of `CoordTask` dependencies — visualises
//!   the same tasks the Kanban edits, ordered by `dependencies` depth.
//! - Replay: R3 unified audit timeline (runs + comments + events + artifacts
//!   + exit journal) per task — the read-side surface of the `task_exit_journal`
//!     builtin tool and `teams.task.trace` RPC.
//! - Workers: live ACP harness session pool (acpx-parity Phase 2) —
//!   surfaces external coding agents (Claude Code / Codex / Gemini /
//!   custom) as first-class workers visible to humans.
//!
//! A small sidebar lets the user pick between the five sub-views and
//! select the active team for the kanban / plan / replay panes.

pub mod components;
pub mod kanban;
pub mod overview;
pub mod plan_dag;
pub mod replay;
pub mod workers;

use crate::api::teams::{TeamSummary, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Sub-tab inside the Teams tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamsSubTab {
    Overview,
    Kanban,
    Plan,
    Replay,
    Workers,
}

/// Shared signals for the Teams tab. Created once at the top-level and
/// consumed via context by overview, kanban, and the sidebar.
#[derive(Clone, Copy)]
pub struct TeamsTabState {
    pub sub_tab: RwSignal<TeamsSubTab>,
    pub teams: RwSignal<Vec<TeamSummary>>,
    pub selected_team_id: RwSignal<Option<String>>,
}

#[component]
#[must_use]
pub fn TeamsView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let tab_state = expect_context::<TeamsTabState>();

    // Initial + reconnect load of the team list. Each successful load keeps the
    // active selection if still present, otherwise falls back to the first team.
    Effect::new(move |_| {
        if dash.is_connected.get() {
            spawn_local(async move {
                if let Ok(list) = TeamsApi::list(&dash).await {
                    // Post-`.await`: see `crate::disposed_reads`. Same shape as
                    // the phone leaf, which is verbatim from this view.
                    let Some(current) = tab_state.selected_team_id.try_get_untracked() else {
                        return;
                    };
                    let keep = current.filter(|id| list.iter().any(|t| &t.id == id));
                    let new_sel = keep.or_else(|| list.first().map(|t| t.id.clone()));
                    tab_state.teams.set(list);
                    tab_state.selected_team_id.set(new_sel);
                }
            });
        } else {
            tab_state.teams.set(Vec::new());
            tab_state.selected_team_id.set(None);
        }
    });

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            {move || match tab_state.sub_tab.get() {
                TeamsSubTab::Overview => view! { <overview::OverviewView /> }.into_any(),
                TeamsSubTab::Kanban => view! { <kanban::KanbanView /> }.into_any(),
                TeamsSubTab::Plan => view! { <plan_dag::PlanDagView /> }.into_any(),
                TeamsSubTab::Replay => view! { <replay::ReplayView /> }.into_any(),
                TeamsSubTab::Workers => view! { <workers::WorkersView /> }.into_any(),
            }}
        </div>
    }
}

/// Sidebar for the Teams tab — sub-tab selector + team dropdown.
#[component]
#[must_use]
pub fn TeamsSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let tab_state = expect_context::<TeamsTabState>();

    view! {
        <div class="flex flex-col h-full">
            <div class="px-3 py-3">
                <components::team_selector::TeamSelector />
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-1">
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.overview).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Overview
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.kanban).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Kanban
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.plan).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Plan
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.replay).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Replay
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.workers).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Workers
                />
            </nav>
        </div>
    }
}

#[component]
fn SubTabButton(
    label: Signal<String>,
    current: RwSignal<TeamsSubTab>,
    target: TeamsSubTab,
) -> impl IntoView {
    let is_active = move || current.get() == target;
    let on_click = move |_| current.set(target);

    view! {
        <button
            on:click=on_click
            class=move || {
                if is_active() {
                    "nav-tile-active w-full flex items-center px-3 py-2 rounded-lg text-sm"
                } else {
                    "nav-tile w-full flex items-center px-3 py-2 rounded-lg text-sm"
                }
            }
        >
            <span>{label}</span>
        </button>
    }
}
