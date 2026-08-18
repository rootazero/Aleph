//! Native iPhone Dashboard screens. Mirrors the phone Chat/Memory/Agents
//! drill-down pattern: a menu landing (`/dashboard`) whose rows mirror the
//! desktop `DashboardSidebar`, each drilling into a full-screen leaf that
//! reuses the existing desktop view (Home / AgentTrace / TasksView / Logs /
//! RuntimesView / UsageView) mounted inside a `PhoneShell` with a back button.
//! Wide interaction on those dense views is deferred (Canvas precedent); this
//! batch only builds the no-split navigation chrome. I/O-only (R4): the menu
//! navigates; leaves reuse the views' own (app-wide context) data.
//!
//! Every leaf passes `wrapped=true`: its child is a *desktop* page that brings
//! its own gutters and its own inner scroll, so the shell must not add a second
//! set, and the `.phone-wrapped` shim (`styles/ios.css`) has to be in scope to
//! stack the desktop columns. See `PhoneShell`'s doc for the full contract.

pub mod menu;

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::i18n::t_string;
use crate::platform::phone::shell::PhoneShell;
use crate::views::agent_trace::AgentTrace;
use crate::views::home::Home;
use crate::views::logs::Logs;
use crate::views::runtimes::RuntimesView;
use crate::views::subagent_tree::SubagentTree;
use crate::views::tasks::TasksView;
use crate::views::usage::UsageView;

use self::menu::PhoneDashboardMenu;

/// Which phone Dashboard screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashScreen {
    Menu,
    Overview,
    Trace,
    Subagents,
    Tasks,
    Logs,
    Runtimes,
    Usage,
}

/// Map a `/dashboard…` path to its phone screen. Trailing slashes are
/// normalized; legacy/unknown sub-paths (`/dashboard/cron`, `/dashboard/memory`)
/// fall back to the menu since the phone doesn't surface them.
///
/// Note the asymmetry with the desktop sidebar: there Overview *is* `/dashboard`,
/// while on the phone `/dashboard` is the sections menu and Overview lives one
/// level down at `/dashboard/overview`. `phone_leaves_match_desktop_sidebar`
/// below pins the rest of the mapping to the sidebar so a leaf added to one
/// side can't stay missing on the other — which is exactly how Subagents came
/// to be desktop-only.
#[must_use]
pub(crate) fn screen_for_path(path: &str) -> DashScreen {
    match path.trim_end_matches('/') {
        "/dashboard" | "" => DashScreen::Menu,
        "/dashboard/overview" => DashScreen::Overview,
        "/dashboard/trace" => DashScreen::Trace,
        "/dashboard/subagents" => DashScreen::Subagents,
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
    let i18n = crate::i18n::use_i18n();
    let location = use_location();
    move || {
        match screen_for_path(&location.pathname.get()) {
        DashScreen::Menu => view! { <PhoneDashboardMenu/> }.into_any(),
        DashScreen::Overview => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.overview) back="/dashboard" back_label="Dashboard" wrapped=true>
                <Home/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Trace => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.agent_trace) back="/dashboard" back_label="Dashboard" wrapped=true>
                <AgentTrace/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Subagents => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.subagents) back="/dashboard" back_label="Dashboard" wrapped=true>
                <SubagentTree/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Tasks => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.scheduled_tasks) back="/dashboard" back_label="Dashboard" wrapped=true>
                <TasksView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Logs => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.server_logs) back="/dashboard" back_label="Dashboard" wrapped=true>
                <Logs/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Runtimes => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.runtimes) back="/dashboard" back_label="Dashboard" wrapped=true>
                <RuntimesView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Usage => view! {
            <PhoneShell title=t_string!(i18n, dashboard.phone.usage) back="/dashboard" back_label="Dashboard" wrapped=true>
                <UsageView/>
            </PhoneShell>
        }
        .into_any(),
    }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source of the desktop dashboard nav. Read as text because a host test
    /// cannot mount a Leptos view — the same reason `queue_row_key` is pinned
    /// with `include_str!`. Asserting against the rendered component is not an
    /// option here, so asserting against its source is the only available grip.
    const DESKTOP_SIDEBAR_SRC: &str = include_str!("../../../components/dashboard_sidebar.rs");

    /// Sidebar hrefs under `/dashboard`, in source order.
    fn desktop_sidebar_hrefs() -> Vec<&'static str> {
        DESKTOP_SIDEBAR_SRC
            .match_indices("href=\"/dashboard")
            .filter_map(|(i, _)| {
                let rest = &DESKTOP_SIDEBAR_SRC[i + "href=\"".len()..];
                rest.find('"').map(|end| &rest[..end])
            })
            .collect()
    }

    /// The phone menu's leaf paths, in menu order.
    fn phone_leaf_paths() -> Vec<&'static str> {
        vec![
            "/dashboard/overview",
            "/dashboard/trace",
            "/dashboard/subagents",
            "/dashboard/tasks",
            "/dashboard/logs",
            "/dashboard/runtimes",
            "/dashboard/usage",
        ]
    }

    #[test]
    fn screen_for_path_maps_all_leaves() {
        assert_eq!(screen_for_path("/dashboard"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/overview"), DashScreen::Overview);
        assert_eq!(screen_for_path("/dashboard/trace"), DashScreen::Trace);
        assert_eq!(
            screen_for_path("/dashboard/subagents"),
            DashScreen::Subagents
        );
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

    /// Sanity: the source scrape found the sidebar at all. Without this the two
    /// assertions below would pass vacuously if the extraction ever broke —
    /// the classic "guard that no longer guards anything" failure.
    #[test]
    fn desktop_sidebar_source_is_readable() {
        let hrefs = desktop_sidebar_hrefs();
        assert!(
            hrefs.len() >= 5,
            "only found {} dashboard hrefs in the sidebar source — the scrape broke",
            hrefs.len()
        );
        assert!(hrefs.contains(&"/dashboard"), "overview href not found");
    }

    /// Every desktop dashboard section is reachable on the phone. Subagents was
    /// added to the desktop sidebar and never to the phone menu; falling back to
    /// `Menu` made that silent — the row simply did not exist, and tapping the
    /// URL bounced you to the menu. `/dashboard` is the one legitimate
    /// asymmetry: it is Overview on desktop and the sections menu on phone.
    #[test]
    fn every_desktop_dashboard_section_has_a_phone_leaf() {
        for href in desktop_sidebar_hrefs() {
            if href == "/dashboard" {
                continue;
            }
            assert_ne!(
                screen_for_path(href),
                DashScreen::Menu,
                "desktop dashboard section {href} has no phone leaf"
            );
        }
    }

    /// …and no phone leaf points at a section the desktop no longer has.
    #[test]
    fn phone_leaves_match_desktop_sidebar() {
        let desktop = desktop_sidebar_hrefs();
        for leaf in phone_leaf_paths() {
            if leaf == "/dashboard/overview" {
                // Desktop lists Overview as `/dashboard`; see above.
                continue;
            }
            assert!(
                desktop.contains(&leaf),
                "phone dashboard leaf {leaf} is not a desktop sidebar section"
            );
        }
    }

    /// The menu component really renders a row per leaf. `screen_for_path`
    /// knowing about a leaf is not the same as the user being able to reach it —
    /// that was the whole Subagents failure. Source assertion for the same
    /// reason as `DESKTOP_SIDEBAR_SRC`.
    #[test]
    fn phone_menu_renders_a_row_for_every_leaf() {
        const MENU_SRC: &str = include_str!("menu.rs");
        for leaf in phone_leaf_paths() {
            assert!(
                MENU_SRC.contains(&format!("go(\"{leaf}\")")),
                "phone dashboard menu has no row navigating to {leaf}"
            );
        }
    }
}
