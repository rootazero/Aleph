// core/ui/control_plane/src/components/sidebar/sidebar.rs
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use super::sidebar_item::SidebarItem;
use crate::context::DashboardState;
use crate::components::sidebar::SidebarMode;

#[component]
pub fn Sidebar() -> impl IntoView {
    let location = use_location();
    let state = expect_context::<DashboardState>();

    // 默认：根据路由自动判断
    let auto_mode = move || {
        if location.pathname.get().starts_with("/settings") {
            SidebarMode::Narrow
        } else {
            SidebarMode::Wide
        }
    };

    // 最终模式：用户覆盖 > 自动判断
    let mode = move || {
        state.sidebar_mode_override.get()
            .unwrap_or_else(|| auto_mode())
    };

    view! {
        <aside class=move || {
            let base = "border-r border-slate-800 bg-slate-900/50 backdrop-blur-xl flex flex-col transition-all duration-300";
            match mode() {
                SidebarMode::Narrow => format!("{} w-16 items-center", base),
                SidebarMode::Wide => format!("{} w-64", base),
            }
        }>
            // Logo 区域（窄模式下只显示图标）
            <LogoSection mode=mode />

            // Navigation
            <nav class="flex-1 px-4 py-4 space-y-2">
                <SidebarItem href="/" label="Dashboard" mode=mode>
                    <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    <polyline points="9 22 9 12 15 12 15 22" />
                </SidebarItem>
                <SidebarItem href="/trace" label="Agent Trace" mode=mode alert_key="agent.trace">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </SidebarItem>
                <SidebarItem href="/status" label="System Health" mode=mode alert_key="system.health">
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
                <SidebarItem href="/memory" label="Memory Vault" mode=mode alert_key="memory.status">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </SidebarItem>
            </nav>

            // Bottom Actions
            <div class="p-4 border-t border-slate-800">
                <A href="/settings" attr:class="flex items-center gap-3 px-3 py-2 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 transition-all duration-200">
                    <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="12" cy="12" r="3" />
                        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
                    </svg>
                    <span class="text-sm font-medium">"Settings"</span>
                </A>
            </div>
        </aside>
    }
}

#[component]
fn LogoSection(mode: impl Fn() -> SidebarMode + 'static + Copy + Send) -> impl IntoView {
    view! {
        <div class=move || {
            match mode() {
                SidebarMode::Wide => "p-6 flex items-center gap-3",
                SidebarMode::Narrow => "p-4 flex items-center justify-center",
            }
        }>
            <div class="w-8 h-8 bg-gradient-to-br from-indigo-500 to-purple-600 rounded-lg flex items-center justify-center shadow-lg shadow-indigo-500/20">
                <span class="text-white font-bold text-xl">"A"</span>
            </div>
            {move || match mode() {
                SidebarMode::Wide => Some(view! {
                    <h1 class="text-xl font-semibold tracking-tight">"Aleph Hub"</h1>
                }),
                SidebarMode::Narrow => None,
            }}
        </div>
    }
}
