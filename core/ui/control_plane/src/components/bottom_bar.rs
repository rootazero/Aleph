// core/ui/control_plane/src/components/bottom_bar.rs
//
// Mobile bottom navigation bar with Chat, Dashboard, and Settings tabs.
// Active tab detection based on current route path.
//
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

/// Panel mode derived from the current route path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Chat,
    Dashboard,
    Settings,
}

impl PanelMode {
    /// Determine panel mode from a URL path.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
    }
}

#[component]
pub fn BottomBar() -> impl IntoView {
    let location = use_location();

    let active_mode = move || {
        let path = location.pathname.get();
        PanelMode::from_path(&path)
    };

    view! {
        <nav class="h-12 bg-sidebar border-t border-border flex justify-around items-center">
            <BottomBarItem
                href="/"
                label="Chat"
                mode=PanelMode::Chat
                active_mode=Signal::derive(active_mode)
            >
                <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
            </BottomBarItem>

            <BottomBarItem
                href="/dashboard"
                label="Dashboard"
                mode=PanelMode::Dashboard
                active_mode=Signal::derive(active_mode)
            >
                <rect x="3" y="3" width="7" height="7"/>
                <rect x="14" y="3" width="7" height="7"/>
                <rect x="14" y="14" width="7" height="7"/>
                <rect x="3" y="14" width="7" height="7"/>
            </BottomBarItem>

            <BottomBarItem
                href="/settings"
                label="Settings"
                mode=PanelMode::Settings
                active_mode=Signal::derive(active_mode)
            >
                <circle cx="12" cy="12" r="3"/>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
            </BottomBarItem>
        </nav>
    }
}

#[component]
fn BottomBarItem(
    href: &'static str,
    label: &'static str,
    mode: PanelMode,
    active_mode: Signal<PanelMode>,
    children: Children,
) -> impl IntoView {
    let is_active = move || active_mode.get() == mode;

    view! {
        <A href=href attr:class=move || {
            if is_active() {
                "flex flex-col items-center justify-center text-sidebar-accent"
            } else {
                "flex flex-col items-center justify-center text-text-tertiary hover:text-text-secondary"
            }
        }>
            <svg
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
            >
                {children()}
            </svg>
            <span class="text-[10px] font-medium">{label}</span>
        </A>
    }
}
