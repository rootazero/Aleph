// core/ui/control_plane/src/components/sidebar/sidebar_item.rs
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn SidebarItem(
    href: &'static str,
    label: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class="flex items-center gap-3 px-3 py-2 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800/50 transition-all duration-200 group active:scale-[0.98]"
        >
            <svg width="20" height="20" attr:class="w-5 h-5 group-hover:scale-110 transition-transform duration-200" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                {children()}
            </svg>
            <span class="text-sm font-medium">{label}</span>
        </A>
    }
}
