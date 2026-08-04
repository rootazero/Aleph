//! Native iPhone Teams screens. Mirrors the phone Dashboard drill-down pattern:
//! a menu landing (`/teams`) whose rows mirror the desktop `TeamsSidebar`
//! (team selector + Overview / Kanban / Plan / Replay / Workers), each drilling
//! into a full-screen leaf that reuses the existing desktop sub-view mounted in
//! a `PhoneShell` with a back button.
//!
//! Every leaf passes `wrapped=true`, for the same reason every `PhoneDashboard`
//! leaf does: its child is a *desktop* page body that brings its own `px-6`
//! gutters and its own `aleph-content-top` macOS traffic-light inset. Stacked on
//! the shell's own 16 px that is 40 px of gutter on a 390 px screen, plus an
//! inset for window controls that do not exist inside this shell.
//!
//! What each leaf needed beyond that, measured rather than assumed:
//!   * **Kanban** — nothing. `KanbanBoard` sizes its columns with an inline
//!     `repeat(auto-fit, minmax(220px, 1fr))`, so 390 px already resolves to one
//!     column. (Inline style also means no stylesheet shim could have changed
//!     it — a `.phone-wrapped .grid` rule would have been silently ignored.)
//!   * **Plan DAG** — nothing. An `overflow-auto` SVG canvas; none of the
//!     `.phone-wrapped` shim rules match it, so `wrapped` only widens it.
//!   * **Replay** — it is a master-detail and was the only one in the tree that
//!     never said so, pairing a bare `w-72` with a `flex-1`. `wrapped` alone
//!     does nothing for that; it now carries the shared `aleph-md` classes, so
//!     the panes stack here *and* on any desktop window under 720 px.
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
                    // Post-`.await`: this screen can be gone (see
                    // `crate::disposed_reads`). `tab_state` is owned by
                    // `AppContent` and outlives it, but the rule is uniform.
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

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        TeamScreen::Menu => view! { <PhoneTeamsMenu/> }.into_any(),
        TeamScreen::Overview => view! {
            <PhoneShell title="Overview" back="/teams" back_label="Teams" wrapped=true>
                <OverviewView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Kanban => view! {
            <PhoneShell title="Kanban" back="/teams" back_label="Teams" wrapped=true>
                <KanbanView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Plan => view! {
            <PhoneShell title="Plan" back="/teams" back_label="Teams" wrapped=true>
                <PlanDagView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Replay => view! {
            <PhoneShell title="Replay" back="/teams" back_label="Teams" wrapped=true>
                <ReplayView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Workers => view! {
            <PhoneShell title="Workers" back="/teams" back_label="Teams" wrapped=true>
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

    /// Only the production half — the RED fixture below is itself a bare shell.
    fn production_half(src: &str) -> &str {
        src.split("#[cfg").next().unwrap_or(src)
    }

    /// Per-shell, not a whole-file count: the failure this pins is *one* leaf
    /// added later without the flag, which no "does any leaf have it" check
    /// would see. Pairing the two tokens on the same line also keeps the check
    /// out of reach of prose — comparing two file-wide counts made this guard
    /// fail the moment the module doc above said `wrapped=true` in a sentence.
    fn every_leaf_asks_for_wrapped_chrome(src: &str) -> bool {
        let shells: Vec<&str> = production_half(src)
            .lines()
            .filter(|l| l.contains("<PhoneShell "))
            .collect();
        !shells.is_empty() && shells.iter().all(|l| l.contains("wrapped=true"))
    }

    #[test]
    fn every_teams_leaf_wraps_its_desktop_body() {
        assert!(
            every_leaf_asks_for_wrapped_chrome(include_str!("mod.rs")),
            "a Teams leaf mounts a desktop page body without `wrapped=true` — it \
             keeps the shell's 16 px on top of the page's own px-6, and keeps an \
             `aleph-content-top` inset for window controls this shell does not have"
        );
    }

    #[test]
    fn wrapped_check_rejects_a_bare_shell() {
        let before = r#"
            TeamScreen::Kanban => view! {
                <PhoneShell title="Kanban" back="/teams" back_label="Teams">
                    <KanbanView/>
                </PhoneShell>
            }.into_any(),
        "#;
        assert!(!every_leaf_asks_for_wrapped_chrome(before));
    }

    /// `wrapped=true` does nothing for Replay on its own: the pane split is a
    /// hand-rolled `w-72` + `flex-1`, and the `.phone-wrapped` shim only knows
    /// how to stack the shared `aleph-md` trio. Losing any one of the three
    /// classes silently restores a 288 px list on a 390 px screen — no error,
    /// no failing render, just a ~102 px trace pane.
    #[test]
    fn the_replay_split_is_wired_to_the_shared_stacking_rule() {
        const REPLAY: &str = include_str!("../../wide/views/teams/replay.rs");
        for class in ["aleph-md\"", "aleph-md-list\"", "aleph-md-detail\""] {
            assert!(
                REPLAY.contains(class),
                "replay.rs lost `{class}` — its panes no longer stack under 720 px"
            );
        }
        const TAILWIND: &str = include_str!("../../../../styles/tailwind.css");
        assert!(
            TAILWIND.contains(".aleph-md >"),
            "the stacking rule itself is gone from tailwind.css — every \
             master-detail page in the tree is unstacked, not just Replay"
        );
    }
}
