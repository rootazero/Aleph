//! Native iPhone Teams screens. Mirrors the phone Dashboard drill-down pattern:
//! a menu landing (`/teams`) whose rows mirror the desktop `TeamsSidebar`
//! (team selector + Overview / Kanban / Plan / Replay / Workers), each drilling
//! into a full-screen leaf that reuses the existing desktop sub-view mounted in
//! a `PhoneShell` with a back button. Wide interaction on the dense leaves
//! (Kanban board / Plan DAG / Replay split) is deferred (Canvas precedent);
//! this batch only builds the no-split navigation chrome.
//!
//! Unlike PhoneDashboard, this router OWNS `TeamsTabState` — the five leaves all
//! read `selected_team_id` from it — and mirrors the desktop `TeamsView`'s
//! connect-gated team-list load. State at the router level keeps the selection
//! alive across menu↔leaf navigation. I/O-only (R4): rows navigate; the load
//! reuses the existing `TeamsApi`.

pub mod menu;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::teams::{
    kanban::KanbanView, overview::OverviewView, plan_dag::PlanDagView, replay::ReplayView,
    workers::WorkersView, TeamsSubTab, TeamsTabState,
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

/// Phone Teams router. Owns `TeamsTabState` (the five leaves read
/// `selected_team_id` from it) and mirrors the desktop `TeamsView` connect-gated
/// team-list load, then dispatches the menu (`/teams`) or a full-screen leaf
/// (`/teams/{leaf}`). State lives at the router so the selection survives
/// menu↔leaf navigation.
#[component]
#[must_use]
pub fn PhoneTeams() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let tab_state = TeamsTabState {
        sub_tab: RwSignal::new(TeamsSubTab::Overview),
        teams: RwSignal::new(Vec::new()),
        selected_team_id: RwSignal::new(None),
    };
    provide_context(tab_state);

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
