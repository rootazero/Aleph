//
// SidebarItem component with real-time alert display.
// Always renders in wide mode (icon + label).
//
use crate::components::ui::StatusBadge;
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

#[component]
pub fn SidebarItem(
    href: &'static str,
    label: Signal<String>,
    children: Children,
    #[prop(optional)] alert_key: Option<&'static str>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let location = use_location();

    let is_active = move || {
        let path = location.pathname.get();
        if href == "/" || href == "/dashboard" {
            path == href
        } else {
            path.starts_with(href)
        }
    };

    let alert = Signal::derive(move || alert_key.and_then(|key| state.get_alert(key)));

    view! {
        <A href=href attr:class=move || {
            if is_active() {
                "nav-tile-active relative group flex items-center gap-3 px-3 py-2 rounded-lg"
            } else {
                "nav-tile relative group flex items-center gap-3 px-3 py-2 rounded-lg"
            }
        }>

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

            // Text label (always wide mode)
            <span class="text-sm font-medium">{move || label.get()}</span>
        </A>
    }
}
