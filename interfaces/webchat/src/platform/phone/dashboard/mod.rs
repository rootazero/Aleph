//! Native iPhone Dashboard screens. Mirrors the phone Chat/Memory/Agents
//! drill-down pattern: a menu landing (`/dashboard`) whose rows mirror the
//! desktop `DashboardSidebar`, each drilling into a full-screen leaf that
//! reuses the existing desktop view (Home / AgentTrace / TasksView / Logs /
//! RuntimesView / UsageView) mounted inside a `PhoneShell` with a back button.
//! Wide interaction on those dense views is deferred (Canvas precedent); this
//! batch only builds the no-split navigation chrome. I/O-only (R4): the menu
//! navigates; leaves reuse the views' own (app-wide context) data.

pub mod menu;

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::platform::phone::shell::PhoneShell;
use crate::views::agent_trace::AgentTrace;
use crate::views::home::Home;
use crate::views::logs::Logs;
use crate::views::runtimes::RuntimesView;
use crate::views::tasks::TasksView;
use crate::views::usage::UsageView;

use self::menu::PhoneDashboardMenu;

/// Which phone Dashboard screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashScreen {
    Menu,
    Overview,
    Trace,
    Tasks,
    Logs,
    Runtimes,
    Usage,
}

/// Map a `/dashboard…` path to its phone screen. Trailing slashes are
/// normalized; legacy/unknown sub-paths (`/dashboard/cron`, `/dashboard/memory`)
/// fall back to the menu since the phone doesn't surface them.
#[must_use]
pub(crate) fn screen_for_path(path: &str) -> DashScreen {
    match path.trim_end_matches('/') {
        "/dashboard" | "" => DashScreen::Menu,
        "/dashboard/overview" => DashScreen::Overview,
        "/dashboard/trace" => DashScreen::Trace,
        "/dashboard/tasks" => DashScreen::Tasks,
        "/dashboard/logs" => DashScreen::Logs,
        "/dashboard/runtimes" => DashScreen::Runtimes,
        "/dashboard/usage" => DashScreen::Usage,
        _ => DashScreen::Menu,
    }
}

/// Phone Dashboard router. Pure path dispatch — no owned state, since each leaf
/// view carries its own data subscriptions from app-wide context. Renders the
/// menu at `/dashboard` or a full-screen leaf at `/dashboard/{leaf}`.
#[component]
#[must_use]
pub fn PhoneDashboard() -> impl IntoView {
    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        DashScreen::Menu => view! { <PhoneDashboardMenu/> }.into_any(),
        DashScreen::Overview => view! {
            <PhoneShell title="Overview" back="/dashboard" back_label="Dashboard">
                <Home/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Trace => view! {
            <PhoneShell title="Agent Trace" back="/dashboard" back_label="Dashboard">
                <AgentTrace/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Tasks => view! {
            <PhoneShell title="Scheduled Tasks" back="/dashboard" back_label="Dashboard">
                <TasksView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Logs => view! {
            <PhoneShell title="Server Logs" back="/dashboard" back_label="Dashboard">
                <Logs/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Runtimes => view! {
            <PhoneShell title="Runtimes" back="/dashboard" back_label="Dashboard">
                <RuntimesView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Usage => view! {
            <PhoneShell title="Usage" back="/dashboard" back_label="Dashboard">
                <UsageView/>
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
        assert_eq!(screen_for_path("/dashboard"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/overview"), DashScreen::Overview);
        assert_eq!(screen_for_path("/dashboard/trace"), DashScreen::Trace);
        assert_eq!(screen_for_path("/dashboard/tasks"), DashScreen::Tasks);
        assert_eq!(screen_for_path("/dashboard/logs"), DashScreen::Logs);
        assert_eq!(screen_for_path("/dashboard/runtimes"), DashScreen::Runtimes);
        assert_eq!(screen_for_path("/dashboard/usage"), DashScreen::Usage);
    }

    #[test]
    fn screen_for_path_legacy_and_unknown_fall_back_to_menu() {
        assert_eq!(screen_for_path("/dashboard/cron"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/memory"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/bogus"), DashScreen::Menu);
    }
}
