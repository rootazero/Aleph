// core/ui/control_plane/src/components/sidebar/sidebar_item.rs
//
// SidebarItem component with real-time alert display.
//
// Alert Integration:
// - Subscribes to DashboardState.alerts via Signal::derive()
// - Reactively displays StatusBadge when alert exists
// - Shows Tooltip with alert details in narrow mode
// - Alert state is updated by WebSocket events from Gateway
//
use leptos::prelude::*;
use leptos_router::components::A;
use crate::context::DashboardState;
use crate::components::sidebar::SidebarMode;
use crate::components::ui::{Tooltip, StatusBadge};

#[component]
pub fn SidebarItem(
    href: &'static str,
    label: &'static str,
    children: Children,
    mode: impl Fn() -> SidebarMode + 'static + Copy + Send,
    #[prop(optional)] alert_key: Option<&'static str>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Subscribe to alert state as a signal
    let alert = Signal::derive(move || {
        alert_key.and_then(|key| state.get_alert(key))
    });

    view! {
        <A href=href attr:class="relative group flex items-center gap-3 px-3 py-2 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800/50 transition-all duration-200">
            // Icon container (relative for badge positioning)
            <div class="relative flex-shrink-0">
                <svg
                    width="20"
                    height="20"
                    attr:class="w-5 h-5 group-hover:scale-110 transition-transform duration-200"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    {children()}
                </svg>

                // Status badge (if alert exists)
                {move || alert.get().map(|a| view! {
                    <StatusBadge level=a.level count=a.count.unwrap_or(0) />
                })}
            </div>

            // Text label (wide mode) or Tooltip (narrow mode)
            {move || match mode() {
                SidebarMode::Wide => view! {
                    <span class="text-sm font-medium">{label}</span>
                }.into_any(),
                SidebarMode::Narrow => view! {
                    <Tooltip text=label alert=alert position="right" />
                }.into_any(),
            }}
        </A>
    }
}
