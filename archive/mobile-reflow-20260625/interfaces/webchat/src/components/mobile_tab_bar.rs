//! Bottom tab bar — the primary section switcher on mobile viewports.
//!
//! Desktop keeps the left `ModeSidebar` + `NavMenu`; this bar is shown only
//! below the `sm` breakpoint (`hidden max-sm:flex`) and never renders on
//! desktop. Four destinations per the iOS design (Chat / Memory / Agents /
//! Settings); Dashboard / Teams / Extensions stay desktop-only for v1.

use super::mode_sidebar::PanelMode;
use super::nav_menu::{icon_of, label_of, route_of};
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};

/// Primary mobile destinations, in tab order.
const MOBILE_TABS: [PanelMode; 4] = [
    PanelMode::Chat,
    PanelMode::Memory,
    PanelMode::Agents,
    PanelMode::Settings,
];

#[component]
#[must_use]
pub fn MobileTabBar() -> impl IntoView {
    let location = use_location();
    let navigate = use_navigate();
    let i18n = use_i18n();
    let current = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <nav
            class="aleph-mobile-tabbar hidden max-sm:flex fixed inset-x-0 bottom-0 z-40 \
                   glass border-t border-border"
            style="padding-bottom: env(safe-area-inset-bottom)"
        >
            {MOBILE_TABS
                .into_iter()
                .map(|m| {
                    let route = route_of(m);
                    let nav = navigate.clone();
                    let is_active = move || current.get() == m;
                    view! {
                        <button
                            type="button"
                            on:click=move |_| nav(route, Default::default())
                            class=move || {
                                let base = "flex-1 flex flex-col items-center justify-center \
                                            gap-0.5 py-2 min-h-[44px] text-[11px] font-medium";
                                if is_active() {
                                    format!("{base} text-primary")
                                } else {
                                    format!("{base} text-text-tertiary")
                                }
                            }
                        >
                            <svg
                                width="22" height="22" viewBox="0 0 24 24" fill="none"
                                stroke="currentColor" stroke-width="1.8"
                                stroke-linecap="round" stroke-linejoin="round"
                                inner_html=icon_of(m)
                            />
                            <span>{label_of(m, i18n)}</span>
                        </button>
                    }
                })
                .collect::<Vec<_>>()}
        </nav>
    }
}
