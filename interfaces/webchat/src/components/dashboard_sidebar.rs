//
// Dashboard mode sidebar — sub-navigation for dashboard views.
//
use leptos::prelude::*;
use crate::components::sidebar::SidebarItem;
use crate::i18n::*;

#[component]
pub fn DashboardSidebar() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col h-full">
            <div class="px-4 py-3">
                <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">{move || t_string!(i18n, dashboard.sidebar.title).to_string()}</h2>
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-0.5">
                <SidebarItem href="/dashboard" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.overview).to_string())>
                    <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    <polyline points="9 22 9 12 15 12 15 22" />
                </SidebarItem>
                <SidebarItem href="/dashboard/trace" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.agent_trace).to_string()) alert_key="agent.trace">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </SidebarItem>
                <SidebarItem href="/dashboard/memory" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.memory_vault).to_string()) alert_key="memory.status">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </SidebarItem>
                <SidebarItem href="/dashboard/tasks" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.scheduled_tasks).to_string())>
                    <circle cx="12" cy="12" r="10" />
                    <polyline points="12 6 12 12 16 14" />
                </SidebarItem>
                <SidebarItem href="/dashboard/logs" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.server_logs).to_string())>
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                    <polyline points="14 2 14 8 20 8" />
                    <line x1="16" y1="13" x2="8" y2="13" />
                    <line x1="16" y1="17" x2="8" y2="17" />
                    <polyline points="10 9 9 9 8 9" />
                </SidebarItem>
                <SidebarItem href="/dashboard/teams" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.teams).to_string())>
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                </SidebarItem>
            </nav>
        </div>
    }
}
