//
// Left column — the context-aware secondary menu, plus a persistent
// bottom-left section switcher (NavMenu).
//
// In Chat mode the secondary menu is the conversation history; in a
// management section it is that section's sub-navigation. The NavMenu
// pinned to the bottom is how the user moves between sections.
//
use super::agents_sidebar::AgentsSidebar;
use super::chat_sidebar::ChatSidebar;
use super::dashboard_sidebar::DashboardSidebar;
use super::nav_menu::NavMenu;
use super::theme_toggle::ThemeToggle;
use crate::components::settings_sidebar::SETTINGS_GROUPS;
use crate::i18n::use_i18n;
use crate::state::memory::MemoryState;
use crate::views::memory_hub::MemorySidebar;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

/// Panel mode derived from the current route path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Chat,
    Dashboard,
    Memory,
    Agents,
    Teams,
    Projects,
    Extensions,
    Settings,
    /// Phone-only ••• tab landing — a sections menu for the management modes
    /// that aren't primary phone tabs. Desktop never routes here.
    More,
}

impl PanelMode {
    /// Determine the panel mode from a URL path.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/memory") {
            Self::Memory
        } else if path.starts_with("/agents") {
            Self::Agents
        } else if path.starts_with("/teams") {
            Self::Teams
        } else if path.starts_with("/projects") {
            Self::Projects
        } else if path.starts_with("/extensions") {
            Self::Extensions
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/more") {
            Self::More
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
    }

    /// True for the sections reached through the phone More (•••) tab. The
    /// ••• tab stays highlighted while inside any of them (iOS "More"
    /// convention). Phone-only concept; desktop never routes to these via More.
    #[must_use]
    pub const fn under_more(self) -> bool {
        matches!(
            self,
            Self::More | Self::Dashboard | Self::Teams | Self::Extensions
        )
    }
}

#[component]
#[must_use]
pub fn ModeSidebar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <aside class="aleph-sidebar w-64 flex flex-col flex-shrink-0 overflow-hidden">
            // Brand row — ℵ wordmark + theme picker, pinned to the top
            <SidebarBrand />

            // Section-specific secondary menu
            <div class="flex-1 min-h-0 overflow-hidden">
                {move || match mode.get() {
                    PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                    PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                    PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
                    PanelMode::Memory => view! { <MemorySidebar /> }.into_any(),
                    PanelMode::Teams => view! { <crate::views::teams::TeamsSidebar /> }.into_any(),
                    PanelMode::Projects => view! { <crate::components::sidebar::projects::ProjectsSidebar /> }.into_any(),
                    PanelMode::Extensions => view! { <crate::views::extensions::ExtensionsSidebar /> }.into_any(),
                    PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
                    // /more is a phone-only route; desktop never reaches it.
                    PanelMode::More => ().into_any(),
                }}
            </div>

            // Bottom-left section switcher — hidden inside the Aleph Hub
            // (Extensions) full-screen takeover, which exits via the
            // "Back to Chat" guide pinned atop its own category sidebar;
            // the switcher reappears in every other mode.
            {move || (mode.get() != PanelMode::Extensions).then(|| view! { <NavMenu /> })}
        </aside>
    }
}

/// Brand row pinned to the top of the left column — the ℵ wordmark on the
/// left, the theme picker + inline sidebar-collapse button on the right.
/// On macOS the row is padded down (via `.aleph-sidebar-head`) so it clears
/// the overlay traffic lights and the fixed top-left toggle that lives
/// alongside them. On web / Tauri Win/Linux the row hugs the top and carries
/// its own inline collapse button instead.
#[component]
fn SidebarBrand() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    view! {
        // Brand row carries `data-tauri-drag-region=""` so the empty
        // space between the logo and the inline buttons drags the
        // window on macOS Overlay-titlebar windows. Each interactive
        // child (Aleph mark area, ThemeToggle, inline collapse button)
        // marks itself as no-drag so the buttons stay clickable.
        <div
            class="aleph-sidebar-head flex items-center justify-between px-3.5 pb-2.5"
            data-tauri-drag-region=""
        >
            <div
                class="aleph-no-drag flex items-center gap-2.5"
                data-tauri-drag-region="false"
            >
                <div class="aleph-mark w-7 h-7 rounded-xl flex items-center justify-center
                            text-text-inverse">
                    <span class="text-base font-semibold leading-none">"\u{2135}"</span>
                </div>
                <span class="text-[15px] font-semibold tracking-tight">"Aleph"</span>
            </div>
            <div
                class="aleph-no-drag flex items-center gap-1"
                data-tauri-drag-region="false"
            >
                <ThemeToggle />
                // Inline collapse button — visible on web + Tauri Win/Linux.
                // Hidden on macOS via CSS (the fixed top-left toggle next to
                // the overlay traffic lights covers it).
                <button
                    type="button"
                    class="aleph-sidebar-inline-toggle aleph-no-drag"
                    data-tauri-drag-region="false"
                    on:click=move |_| {
                        let s = &mem.sidebar_collapsed;
                        s.set(!s.get());
                    }
                    title="Collapse sidebar"
                    aria-label="Collapse sidebar"
                >
                    <svg
                        width="18" height="18" viewBox="0 0 24 24" fill="none"
                        stroke="currentColor" stroke-width="1.8"
                        stroke-linecap="round" stroke-linejoin="round"
                    >
                        <rect x="3" y="5" width="18" height="14" rx="2.5" />
                        <line x1="9" y1="5" x2="9" y2="19" />
                    </svg>
                </button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::PanelMode;

    #[test]
    fn from_path_classifies_more() {
        assert_eq!(PanelMode::from_path("/more"), PanelMode::More);
        assert_eq!(PanelMode::from_path("/more/"), PanelMode::More);
        // /more must not shadow, nor be shadowed by, the other sections.
        assert_eq!(PanelMode::from_path("/memory"), PanelMode::Memory);
        assert_eq!(PanelMode::from_path("/dashboard"), PanelMode::Dashboard);
        assert_eq!(PanelMode::from_path("/settings"), PanelMode::Settings);
        assert_eq!(PanelMode::from_path("/"), PanelMode::Chat);
    }

    #[test]
    fn under_more_covers_more_sections() {
        for m in [
            PanelMode::More,
            PanelMode::Dashboard,
            PanelMode::Teams,
            PanelMode::Extensions,
        ] {
            assert!(m.under_more(), "{m:?} should be under More");
        }
        for m in [
            PanelMode::Chat,
            PanelMode::Memory,
            PanelMode::Agents,
            PanelMode::Projects,
            PanelMode::Settings,
        ] {
            assert!(!m.under_more(), "{m:?} should not be under More");
        }
    }
}

/// Settings mode sidebar — reuses existing SettingsTab definitions.
/// Includes a client-side text filter at the top — typed query trims
/// groups + tabs by label (case-insensitive). Match against group label
/// shows the full group; match against a tab shows just that tab plus
/// its parent group header for context.
#[component]
fn SettingsSidebar() -> impl IntoView {
    let location = use_location();
    let i18n = use_i18n();
    let filter = RwSignal::new(String::new());

    view! {
        <div class="flex flex-col h-full">
            // Search filter input — sticky so it stays visible when the
            // tab list scrolls.
            <div class="sticky top-0 z-10 bg-surface-base/95 aleph-blur-subtle px-3 pt-3 pb-2 border-b border-border/50">
                <div class="relative">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                         stroke="currentColor" stroke-width="2" stroke-linecap="round"
                         stroke-linejoin="round"
                         class="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary"
                    >
                        <circle cx="11" cy="11" r="8" />
                        <path d="m21 21-4.35-4.35" />
                    </svg>
                    <input
                        type="text"
                        placeholder="Search settings…"
                        class="w-full pl-8 pr-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/60 focus:ring-1 focus:ring-primary/30"
                        prop:value=move || filter.get()
                        on:input=move |ev| filter.set(event_target_value(&ev))
                    />
                </div>
            </div>

            <div class="flex-1 overflow-y-auto">
                {move || {
                    let needle = filter.get().trim().to_lowercase();
                    SETTINGS_GROUPS.iter().filter_map(|group| {
                        let group_label = group.i18n_label(i18n);
                        let group_matches = !needle.is_empty()
                            && group_label.to_lowercase().contains(&needle);

                        // Per-tab visibility: with an empty query everything
                        // shows; otherwise we keep tabs whose label contains
                        // the needle, or all tabs when the group label matches.
                        let visible_tabs: Vec<_> = group.tabs.iter().filter(|tab| {
                            if needle.is_empty() || group_matches {
                                true
                            } else {
                                tab.i18n_label(i18n).to_lowercase().contains(&needle)
                            }
                        }).collect();

                        if visible_tabs.is_empty() {
                            return None;
                        }

                        Some(view! {
                            <div class="px-3 py-2 space-y-0.5">
                                <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                                    {group_label}
                                </h3>
                                {visible_tabs.into_iter().map(|tab| {
                                    let path = tab.path();
                                    let tab_label = tab.i18n_label(i18n);
                                    let icon_svg = tab.icon_svg();
                                    let is_active = {
                                        let location = location.clone();
                                        move || {
                                            let current = location.pathname.get();
                                            if path == "/settings/channels" {
                                                current.starts_with(path)
                                            } else {
                                                current == path
                                            }
                                        }
                                    };

                                    view! {
                                        <A
                                            href=path
                                            attr:class=move || {
                                                if is_active() {
                                                    "nav-tile-active flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                                                } else {
                                                    "nav-tile flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                                                }
                                            }
                                        >
                                            <svg width="18" height="18" viewBox="0 0 24 24" fill="none"
                                                 stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                                 stroke-linejoin="round"
                                                 class=move || {
                                                     if is_active() { "text-sidebar-accent flex-shrink-0" }
                                                     else { "text-text-tertiary flex-shrink-0" }
                                                 }
                                                 inner_html=icon_svg
                                            />
                                            <span>{tab_label}</span>
                                        </A>
                                    }
                                }).collect_view()}
                            </div>
                        })
                    }).collect_view()
                }}
            </div>
        </div>
    }
}
