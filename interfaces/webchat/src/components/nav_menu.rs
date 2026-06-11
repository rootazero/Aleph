//
// Bottom-left section switcher.
//
// A compact button pinned to the foot of the left column. Clicking it
// opens an upward popup to jump between Chat and the management
// sections (Dashboard / Memory / Agents / Teams / Settings). Choosing a
// section navigates the router — the left column then swaps to that
// section's secondary menu and the main area to its content.
//
use super::mode_sidebar::PanelMode;
use crate::i18n::*;
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
fn route_of(mode: PanelMode) -> &'static str {
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
fn icon_of(mode: PanelMode) -> &'static str {
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
pub fn NavMenu() -> impl IntoView {
    let location = use_location();
    let navigate = use_navigate();
    let i18n = use_i18n();
    let open = RwSignal::new(false);

    let current = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <div class="relative border-t border-border p-2 flex-shrink-0">
            // Trigger
            <button
                on:click=move |_| open.update(|v| *v = !*v)
                class=move || {
                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                    if open.get() {
                        format!("{base} nav-tile-active")
                    } else {
                        format!("{base} nav-tile")
                    }
                }
            >
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                     class="flex-shrink-0"
                     inner_html=move || icon_of(current.get())
                />
                <span class="flex-1 text-left font-medium truncate">
                    {move || label_of(current.get(), i18n)}
                </span>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                     class=move || {
                         if open.get() {
                             "flex-shrink-0 text-text-tertiary rotate-180 transition-transform"
                         } else {
                             "flex-shrink-0 text-text-tertiary transition-transform"
                         }
                     }
                >
                    <polyline points="18 15 12 9 6 15" />
                </svg>
            </button>

            // Click-outside catcher
            {move || open.get().then(|| view! {
                <div class="fixed inset-0 z-40" on:click=move |_| open.set(false) />
            })}

            // Popup — opens upward
            <Show when=move || open.get()>
                <div class="glass animate-pop-in absolute bottom-full left-2 right-2 mb-2 z-50
                            rounded-xl border border-border bg-surface-overlay/85 shadow-xl p-1.5 space-y-0.5">
                    {ALL_MODES.into_iter().map(|m| {
                        let route = route_of(m);
                        let nav = navigate.clone();
                        let is_active = move || current.get() == m;
                        view! {
                            <button
                                on:click=move |_| {
                                    open.set(false);
                                    nav(route, Default::default());
                                }
                                class=move || {
                                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                                    if is_active() {
                                        format!("{base} nav-tile-active")
                                    } else {
                                        format!("{base} nav-tile")
                                    }
                                }
                            >
                                <svg width="18" height="18" viewBox="0 0 24 24" fill="none"
                                     stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                     stroke-linejoin="round" class="flex-shrink-0"
                                     inner_html=icon_of(m)
                                />
                                <span class="flex-1 text-left">{label_of(m, i18n)}</span>
                                {move || is_active().then(|| view! {
                                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                                         stroke="currentColor" stroke-width="3" stroke-linecap="round"
                                         stroke-linejoin="round" class="flex-shrink-0">
                                        <polyline points="20 6 9 17 4 12" />
                                    </svg>
                                })}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </Show>
        </div>
    }
}
