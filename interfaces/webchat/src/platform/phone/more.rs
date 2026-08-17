//! Phone More entry (`/more`): the 5th-tab landing — a full-screen sections
//! menu for the management modes that aren't primary phone tabs
//! (Dashboard / Teams / Extensions / Alerts). Each row navigates into that
//! mode; that mode's own phone screen is a separate spec, so until then the
//! target renders the existing desktop layout. Mirrors the `PhoneSettings`
//! landing structure. I/O-only (R4): rows only navigate.
//!
//! Alerts is a leaf of this menu rather than a sixth tab: iOS caps a tab bar at
//! five, and the fifth is already this one. It still gets tab-bar-level
//! visibility, because an alerts entry nobody notices is not an entry — the
//! ••• item carries the unread badge (see [`super::shell::PhoneTabBar`]).

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::NavigateOptions;

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::platform::phone::alerts::PhoneAlerts;
use crate::platform::phone::shell::PhoneShell;
use crate::state::notifications::{unread_count, NotificationsState};

/// Unread badge count shared by this menu's Alerts row and the tab bar item.
///
/// One derivation, two readers. The desktop bell computes the same number the
/// same way; what must not happen is a third spelling of "how many things are
/// waiting", because the disagreement only ever shows up as a badge that says 2
/// over a list that shows 1.
#[must_use]
pub fn alert_badge_count() -> Memo<usize> {
    let dashboard = use_context::<DashboardState>();
    let notif = use_context::<NotificationsState>();
    Memo::new(move |_| {
        let (Some(dashboard), Some(notif)) = (dashboard, notif) else {
            return 0;
        };
        unread_count(&dashboard.alerts.get(), &notif.dismissed.get())
            + dashboard.pending_approvals.get().len()
    })
}

/// Exact match (trailing slash tolerated), not `starts_with`: a prefix test
/// would also claim `/more/alertsomething`, and this router's fallback is the
/// menu, so the mistake would render the wrong screen rather than 404.
#[must_use]
pub fn is_alerts_path(path: &str) -> bool {
    matches!(path.trim_end_matches('/'), "/more/alerts")
}

/// `/more` → the sections menu; `/more/alerts` → the alerts leaf.
#[component]
#[must_use]
pub fn PhoneMore() -> impl IntoView {
    let location = use_location();
    move || {
        if is_alerts_path(&location.pathname.get()) {
            view! { <PhoneAlerts/> }.into_any()
        } else {
            view! { <PhoneMoreMenu/> }.into_any()
        }
    }
}

#[component]
#[must_use]
fn PhoneMoreMenu() -> impl IntoView {
    let navigate = use_navigate();
    let i18n = use_i18n();
    let badge = alert_badge_count();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="More">
            <div class="list">
                <div class="cell" on:click=go("/dashboard")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="3" y="3" width="7" height="7"></rect>
                            <rect x="14" y="3" width="7" height="7"></rect>
                            <rect x="14" y="14" width="7" height="7"></rect>
                            <rect x="3" y="14" width="7" height="7"></rect>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Dashboard"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/teams")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path>
                            <circle cx="9" cy="7" r="4"></circle>
                            <path d="M23 21v-2a4 4 0 0 0-3-3.87"></path>
                            <path d="M16 3.13a4 4 0 0 1 0 7.75"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Teams"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/canvas")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="3" y="3" width="18" height="18" rx="2"></rect>
                            <path d="M7 14c1.5-4 3-4 4.5-1s3 3 5.5-3"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">{t_string!(i18n, nav.canvas).to_string()}</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/extensions")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.2 2.2 0 1 1 0 4.4H2V19a2 2 0 0 0 2 2h3.8v-1.5a2.2 2.2 0 1 1 4.4 0V21H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5z"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Extensions"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/more/alerts")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"></path>
                            <path d="M13.7 21a2 2 0 0 1-3.4 0"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Alerts"</div></div>
                    // Braces required: bare `badge.get() > 0` lets the `view!`
                    // macro read the `>` as the tag close.
                    <Show when=move || { badge.get() > 0 }>
                        <span
                            class="cell-value"
                            style="background:var(--color-danger); color:#fff; border-radius:9999px; min-width:20px; padding:1px 6px; text-align:center; font-size:0.75rem; font-weight:600;"
                        >
                            {move || badge.get().to_string()}
                        </span>
                    </Show>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </PhoneShell>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::mode_sidebar::PanelMode;

    #[test]
    fn alerts_path_matches_exactly() {
        assert!(is_alerts_path("/more/alerts"));
        assert!(is_alerts_path("/more/alerts/"));
        assert!(!is_alerts_path("/more"));
        assert!(!is_alerts_path("/more/"));
        // The prefix trap: this must fall through to the menu, not the leaf.
        assert!(!is_alerts_path("/more/alertsomething"));
        assert!(!is_alerts_path("/more/alerts/extra"));
    }

    /// The route was chosen to sit *under* `/more` precisely so no new
    /// `PanelMode` was needed and the ••• tab keeps its highlight. If the path
    /// ever moves out from under that prefix, the screen still renders — it is
    /// just unreachable, because `MainContent` only mounts `PhoneMore` while the
    /// mode is `More`. Nothing else would say so.
    #[test]
    fn the_alerts_route_stays_under_the_more_tab() {
        assert_eq!(PanelMode::from_path("/more/alerts"), PanelMode::More);
        assert!(PanelMode::from_path("/more/alerts").under_more());
    }

    /// Only the production half — the RED fixture below is a second reader.
    fn production_half(src: &str) -> &str {
        src.split("#[cfg").next().unwrap_or(src)
    }

    /// Both badge readers must call the shared derivation.
    ///
    /// The badge is the whole reason this entry point is discoverable, and it
    /// has two renderers on two different screens (the ••• tab dot and the
    /// Alerts row's count). A second hand-rolled `unread_count(..) + ..` would
    /// compile, look right, and drift the first time either side gains a term —
    /// showing a dot over a list that says there is nothing to see.
    fn reads_the_shared_badge(src: &str) -> bool {
        production_half(src).contains("alert_badge_count()")
    }

    #[test]
    fn both_badge_surfaces_share_one_derivation() {
        assert!(
            reads_the_shared_badge(include_str!("more.rs")),
            "the More menu stopped using the shared badge derivation"
        );
        assert!(
            reads_the_shared_badge(include_str!("shell.rs")),
            "the tab bar stopped using the shared badge derivation — its dot and \
             the Alerts row's count can now disagree"
        );
    }

    #[test]
    fn badge_check_rejects_a_hand_rolled_second_count() {
        let before = r"
            let badge = Memo::new(move |_| {
                unread_count(&dashboard.alerts.get(), &notif.dismissed.get())
            });
        ";
        assert!(!reads_the_shared_badge(before));
    }
}
