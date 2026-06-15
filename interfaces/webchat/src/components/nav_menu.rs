//
// Bottom-left section navigation grid.
//
// A persistent 2-column grid of nav tiles pinned to the foot of the left
// column, one per cross-cutting management section (Dashboard / Memory /
// Agents / Teams). Chat is the default surface (the sidebar itself) and
// Settings is reachable via the header gear, so both are filtered out here.
// Clicking a tile navigates the router — the left column then swaps to that
// section's secondary menu and the main area to its content; the active
// section's tile is highlighted.
//
use super::mode_sidebar::PanelMode;
use crate::i18n::{t_string, Locale, use_i18n};
use leptos::prelude::*;
use leptos_i18n::I18nContext;
use leptos_router::hooks::{use_location, use_navigate};

/// Sections offered in the switcher, in display order.
const ALL_MODES: [PanelMode; 6] = [
    PanelMode::Chat,
    PanelMode::Dashboard,
    PanelMode::Memory,
    PanelMode::Agents,
    PanelMode::Teams,
    PanelMode::Settings,
];

/// Default route a section navigates to.
const fn route_of(mode: PanelMode) -> &'static str {
    match mode {
        PanelMode::Chat => "/chat",
        PanelMode::Dashboard => "/dashboard",
        PanelMode::Memory => "/memory",
        PanelMode::Agents => "/agents",
        PanelMode::Teams => "/teams",
        PanelMode::Settings => "/settings",
    }
}

/// Localized section label.
fn label_of(mode: PanelMode, i18n: I18nContext<Locale>) -> String {
    match mode {
        PanelMode::Chat => t_string!(i18n, nav.chat).to_string(),
        PanelMode::Dashboard => t_string!(i18n, nav.dashboard).to_string(),
        PanelMode::Memory => t_string!(i18n, nav.memory).to_string(),
        PanelMode::Agents => t_string!(i18n, nav.agents).to_string(),
        PanelMode::Teams => t_string!(i18n, nav.teams).to_string(),
        PanelMode::Settings => t_string!(i18n, nav.settings).to_string(),
    }
}

/// Inline SVG body for a section's icon (24×24 viewBox, stroked).
const fn icon_of(mode: PanelMode) -> &'static str {
    match mode {
        PanelMode::Chat => {
            r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#
        }
        PanelMode::Dashboard => {
            r#"<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/>"#
        }
        PanelMode::Memory => {
            r#"<circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="16"/><line x1="8" y1="12" x2="16" y2="12"/><circle cx="8" cy="8" r="1.5"/><circle cx="16" cy="8" r="1.5"/><circle cx="8" cy="16" r="1.5"/><circle cx="16" cy="16" r="1.5"/>"#
        }
        PanelMode::Agents => {
            r#"<circle cx="12" cy="8" r="4"/><path d="M6 21v-2a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v2"/><line x1="12" y1="2" x2="12" y2="4"/>"#
        }
        PanelMode::Teams => {
            r#"<path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>"#
        }
        PanelMode::Settings => {
            r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>"#
        }
    }
}

#[component]
#[must_use]
pub fn NavMenu() -> impl IntoView {
    let location = use_location();
    let navigate = use_navigate();
    let i18n = use_i18n();

    let current = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <nav class="grid grid-cols-2 gap-1 p-2 border-t border-border flex-shrink-0">
            {ALL_MODES.into_iter()
                // Chat is the default surface (the sidebar itself); Settings is
                // reachable via the header gear. Show the cross-cutting modes.
                .filter(|m| !matches!(m, PanelMode::Chat | PanelMode::Settings))
                .map(|mode| {
                    let nav = navigate.clone();
                    let route = route_of(mode);
                    let label = label_of(mode, i18n);
                    let icon = icon_of(mode);
                    let is_active = move || current.get() == mode;
                    view! {
                        <button
                            on:click=move |_| nav(route, Default::default())
                            class=move || {
                                let base = "flex items-center gap-2 px-2 py-1.5 rounded-lg text-sm";
                                if is_active() {
                                    format!("{base} nav-tile-active")
                                } else {
                                    format!("{base} nav-tile")
                                }
                            }
                        >
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
                                 stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                 stroke-linejoin="round" class="flex-shrink-0"
                                 inner_html=icon
                            />
                            <span class="truncate">{label}</span>
                        </button>
                    }
                })
                .collect::<Vec<_>>()}
        </nav>
    }
}
