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
use crate::components::settings_sidebar::SETTINGS_GROUPS;
use crate::i18n::*;
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
    Settings,
}

impl PanelMode {
    /// Determine the panel mode from a URL path.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/memory") {
            Self::Memory
        } else if path.starts_with("/agents") {
            Self::Agents
        } else if path.starts_with("/teams") {
            Self::Teams
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
    }
}

#[component]
pub fn ModeSidebar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <aside class="w-64 border-r border-border bg-sidebar flex flex-col flex-shrink-0 overflow-hidden">
            // Section-specific secondary menu
            <div class="flex-1 min-h-0 overflow-hidden">
                {move || match mode.get() {
                    PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                    PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                    PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
                    PanelMode::Memory => view! { <MemorySidebar /> }.into_any(),
                    PanelMode::Teams => view! { <crate::views::teams::TeamsSidebar /> }.into_any(),
                    PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
                }}
            </div>

            // Persistent bottom-left section switcher
            <NavMenu />
        </aside>
    }
}

/// Memory mode has no sub-navigation — the knowledge graph is a single
/// canvas. Show a minimal header so the column stays consistent.
#[component]
fn MemorySidebar() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex flex-col h-full p-4">
            <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                {move || t_string!(i18n, nav.memory).to_string()}
            </h2>
            <p class="mt-2 text-xs text-text-tertiary leading-relaxed">
                "知识图谱 — 在右侧画布中拖拽、缩放并探索记忆节点。"
            </p>
        </div>
    }
}

/// Settings mode sidebar — reuses existing SettingsTab definitions.
#[component]
fn SettingsSidebar() -> impl IntoView {
    let location = use_location();
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col h-full overflow-y-auto">
            {move || SETTINGS_GROUPS.iter().map(|group| {
                let group_label = group.i18n_label(i18n);
                view! {
                    <div class="px-3 py-2 space-y-0.5">
                        <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group_label}
                        </h3>
                        {group.tabs.iter().map(|tab| {
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
                                            "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 bg-sidebar-active text-sidebar-accent font-medium"
                                        } else {
                                            "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
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
                }
            }).collect_view()}
        </div>
    }
}
