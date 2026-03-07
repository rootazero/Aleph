// apps/panel/src/components/dashboard_sidebar.rs
//
// Dashboard mode sidebar — sub-navigation for dashboard views.
//
use leptos::prelude::*;
use crate::components::sidebar::SidebarItem;

#[component]
pub fn DashboardSidebar() -> impl IntoView {
    view! {
        <div class="flex flex-col h-full">
            <div class="px-4 py-3">
                <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">"Dashboard"</h2>
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-0.5">
                <SidebarItem href="/dashboard" label="Overview">
                    <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    <polyline points="9 22 9 12 15 12 15 22" />
                </SidebarItem>
                <SidebarItem href="/dashboard/trace" label="Agent Trace" alert_key="agent.trace">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </SidebarItem>
                <SidebarItem href="/dashboard/health" label="System Health" alert_key="system.health">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                    <line x1="9" y1="20" x2="9" y2="23" />
                    <line x1="15" y1="20" x2="15" y2="23" />
                    <line x1="20" y1="9" x2="23" y2="9" />
                    <line x1="20" y1="15" x2="23" y2="15" />
                    <line x1="1" y1="9" x2="4" y2="9" />
                    <line x1="1" y1="15" x2="4" y2="15" />
                </SidebarItem>
                <SidebarItem href="/dashboard/memory" label="Memory Vault" alert_key="memory.status">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </SidebarItem>
                <SidebarItem href="/dashboard/cron" label="Scheduled Tasks">
                    <circle cx="12" cy="12" r="10" />
                    <polyline points="12 6 12 12 16 14" />
                </SidebarItem>
                <SidebarItem href="/dashboard/logs" label="Server Logs">
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                    <polyline points="14 2 14 8 20 8" />
                    <line x1="16" y1="13" x2="8" y2="13" />
                    <line x1="16" y1="17" x2="8" y2="17" />
                    <polyline points="10 9 9 9 8 9" />
                </SidebarItem>
            </nav>
        </div>
    }
}
