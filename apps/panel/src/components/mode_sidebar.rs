// apps/panel/src/components/mode_sidebar.rs
//
// Context-aware sidebar that switches content based on current panel mode.
//
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use super::bottom_bar::PanelMode;
use super::agents_sidebar::AgentsSidebar;
use super::chat_sidebar::ChatSidebar;
use super::dashboard_sidebar::DashboardSidebar;
use crate::components::settings_sidebar::SETTINGS_GROUPS;

#[component]
pub fn ModeSidebar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <aside class="w-64 border-r border-border bg-sidebar flex flex-col flex-shrink-0 overflow-hidden">
            {move || match mode.get() {
                PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
                PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
            }}
        </aside>
    }
}

/// Settings mode sidebar — reuses existing SettingsTab definitions.
#[component]
fn SettingsSidebar() -> impl IntoView {
    let location = use_location();

    view! {
        <div class="flex flex-col h-full overflow-y-auto">
            {SETTINGS_GROUPS.iter().map(|group| {
                view! {
                    <div class="px-3 py-2 space-y-0.5">
                        <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group.label}
                        </h3>
                        {group.tabs.iter().map(|tab| {
                            let path = tab.path();
                            let tab_label = tab.label();
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
                                <a
                                    href=path
                                    class=move || {
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
                                </a>
                            }
                        }).collect_view()}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
