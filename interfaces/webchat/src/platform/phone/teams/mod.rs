//! Native iPhone Teams screens. Mirrors the phone Dashboard drill-down pattern:
//! a menu landing (`/teams`) whose rows mirror the desktop `TeamsSidebar`
//! (team selector + Overview / Kanban / Plan / Replay / Workers), each drilling
//! into a full-screen leaf that reuses the existing desktop sub-view mounted in
//! a `PhoneShell` with a back button. Wide interaction on the dense leaves
//! (Kanban board / Plan DAG / Replay split) is deferred (Canvas precedent);
//! this batch only builds the no-split navigation chrome.
//!
//! `TeamsTabState` is READ from context, never re-provided: the app root is its
//! only owner (see `AppContent`). The five leaves are the desktop components, so
//! they resolve the same context whichever shell mounts them, and the selection
//! survives menu↔leaf navigation because the root outlives every screen here.
//! This router only mirrors the desktop `TeamsView`'s connect-gated team-list
//! load. I/O-only (R4): rows navigate; the load reuses the existing `TeamsApi`.
//!
//! # Why re-providing it crashed the panel
//!
//! A `#[component]` is a plain function call — Leptos gives it no `Owner` of its
//! own (only `#[island]` gets one). So a `provide_context` here landed in the
//! owner of the *form-factor branch* in `MainContent`, the dynamic child that
//! swaps `PhoneTeams` ↔ `TeamsView` when the viewport crosses 640 px.
//!
//! That branch re-runs under `Owner::with_cleanup`, and `reactive_graph`'s
//! `cleanup()` takes `cleanups`, `nodes` and `children` — but **not
//! `contexts`**. The signals died; the context entry pointing at them did not.
//! `TeamsView` was then built under that same owner, `expect_context` handed it
//! the corpse, and `tab_state.sub_tab.get()` panicked mid-`Render::build` with
//! "Tried to access a reactive value that has already been disposed" — followed
//! by its `spawn_local` load hitting the same disposed `selected_team_id`.
//!
//! Only widening crashed: going the other way this component *overwrote* the
//! entry with live signals before anything read it, so the corpse never
//! surfaced. The general rule the guard test below pins: **nothing under
//! `platform/phone/` may provide a context type that `platform/wide/` consumes**
//! — the two branches share an owner, so a shadowing provide is a dangling
//! pointer waiting for a resize.

pub mod menu;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::teams::{
    kanban::KanbanView, overview::OverviewView, plan_dag::PlanDagView, replay::ReplayView,
    workers::WorkersView, TeamsTabState,
};

use self::menu::PhoneTeamsMenu;

/// Which phone Teams screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamScreen {
    Menu,
    Overview,
    Kanban,
    Plan,
    Replay,
    Workers,
}

/// Map a `/teams…` path to its phone screen. Trailing slashes are normalized;
/// the bare mode path and any unknown sub-path fall back to the menu.
#[must_use]
pub(crate) fn screen_for_path(path: &str) -> TeamScreen {
    match path.trim_end_matches('/') {
        "/teams" | "" => TeamScreen::Menu,
        "/teams/overview" => TeamScreen::Overview,
        "/teams/kanban" => TeamScreen::Kanban,
        "/teams/plan" => TeamScreen::Plan,
        "/teams/replay" => TeamScreen::Replay,
        "/teams/workers" => TeamScreen::Workers,
        _ => TeamScreen::Menu,
    }
}

/// Phone Teams router. Reads `TeamsTabState` from the app root (the five leaves
/// read `selected_team_id` from that same context) and mirrors the desktop
/// `TeamsView` connect-gated team-list load, then dispatches the menu (`/teams`)
/// or a full-screen leaf (`/teams/{leaf}`).
#[component]
#[must_use]
pub fn PhoneTeams() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    // Root-owned, never re-provided here — see the module doc for the crash that
    // a second provider caused. `AppContent` provides it unconditionally, so
    // this cannot miss.
    let tab_state = expect_context::<TeamsTabState>();

    // Initial + reconnect load of the team list — verbatim from desktop TeamsView.
    Effect::new(move |_| {
        if dash.is_connected.get() {
            spawn_local(async move {
                if let Ok(list) = TeamsApi::list(&dash).await {
                    let keep = tab_state
                        .selected_team_id
                        .get_untracked()
                        .filter(|id| list.iter().any(|t| &t.id == id));
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

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        TeamScreen::Menu => view! { <PhoneTeamsMenu/> }.into_any(),
        TeamScreen::Overview => view! {
            <PhoneShell title="Overview" back="/teams" back_label="Teams">
                <OverviewView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Kanban => view! {
            <PhoneShell title="Kanban" back="/teams" back_label="Teams">
                <KanbanView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Plan => view! {
            <PhoneShell title="Plan" back="/teams" back_label="Teams">
                <PlanDagView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Replay => view! {
            <PhoneShell title="Replay" back="/teams" back_label="Teams">
                <ReplayView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Workers => view! {
            <PhoneShell title="Workers" back="/teams" back_label="Teams">
                <WorkersView/>
            </PhoneShell>
        }
        .into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_for_path_maps_all_leaves() {
        assert_eq!(screen_for_path("/teams"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/overview"), TeamScreen::Overview);
        assert_eq!(screen_for_path("/teams/kanban"), TeamScreen::Kanban);
        assert_eq!(screen_for_path("/teams/plan"), TeamScreen::Plan);
        assert_eq!(screen_for_path("/teams/replay"), TeamScreen::Replay);
        assert_eq!(screen_for_path("/teams/workers"), TeamScreen::Workers);
    }

    #[test]
    fn screen_for_path_unknown_falls_back_to_menu() {
        assert_eq!(screen_for_path("/teams/bogus"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/overview/extra"), TeamScreen::Menu);
    }
}
