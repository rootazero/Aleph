// core/ui/control_plane/src/components/sidebar/sidebar.rs
//
// Unified flat sidebar: Dashboard (expandable) + Settings groups.
//
// Alert Flow:
// 1. Gateway emits alert events (e.g., "alerts.system.health")
// 2. DashboardState.setup_alert_subscriptions() subscribes to "alerts.**"
// 3. Event handler updates DashboardState.alerts HashMap
// 4. SidebarItem subscribes to alerts via Signal::derive()
// 5. StatusBadge/Tooltip display alert state reactively
//
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use super::sidebar_item::SidebarItem;
use crate::components::settings_sidebar::{SETTINGS_GROUPS, SettingsTab};

/// Theme mode: System, Light, or Dark
#[derive(Debug, Clone, Copy, PartialEq)]
enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let location = use_location();
    let dashboard_expanded = RwSignal::new(true);

    // Auto-expand Dashboard when navigating to dashboard sub-routes
    Effect::new(move || {
        let path = location.pathname.get();
        if path == "/" || path == "/trace" || path == "/status" || path == "/memory" {
            dashboard_expanded.set(true);
        }
    });

    view! {
        <aside class="w-64 border-r border-border bg-sidebar flex flex-col transition-all duration-300">
            // Logo
            <LogoSection />

            // Scrollable navigation area
            <nav class="flex-1 overflow-y-auto px-4 py-4 space-y-4">
                // Dashboard section (expandable)
                <DashboardSection expanded=dashboard_expanded />

                // Settings groups
                {SETTINGS_GROUPS.iter().map(|group| {
                    view! {
                        <SettingsGroupSection label=group.label tabs=group.tabs />
                    }
                }).collect_view()}
            </nav>

            // Bottom: Theme toggle
            <div class="p-4 border-t border-border space-y-1">
                <ThemeToggle />
            </div>
        </aside>
    }
}

/// Dashboard section with expandable sub-items
#[component]
fn DashboardSection(expanded: RwSignal<bool>) -> impl IntoView {
    let location = use_location();
    let dashboard_expanded = expanded;

    let is_dashboard_active = move || {
        let path = location.pathname.get();
        path == "/" || path == "/trace" || path == "/status" || path == "/memory"
    };

    let toggle = move |_| {
        dashboard_expanded.update(|v| *v = !*v);
    };

    view! {
        <div class="space-y-0.5">
            // Dashboard header row (clickable to expand/collapse)
            <button
                on:click=toggle
                class=move || {
                    let base = "flex items-center gap-3 px-3 py-2 rounded-lg w-full text-left transition-all duration-200";
                    if is_dashboard_active() {
                        format!("{} text-sidebar-accent bg-sidebar-active font-medium", base)
                    } else {
                        format!("{} text-text-secondary hover:text-text-primary hover:bg-sidebar-active/50", base)
                    }
                }
            >
                // Expand/collapse chevron
                <svg
                    width="16"
                    height="16"
                    attr:class=move || {
                        let base = "w-4 h-4 transition-transform duration-200 flex-shrink-0";
                        if dashboard_expanded.get() {
                            format!("{} rotate-90", base)
                        } else {
                            base.to_string()
                        }
                    }
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    <polyline points="9 18 15 12 9 6" />
                </svg>

                // Home icon
                <svg width="20" height="20" attr:class="w-5 h-5 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    <polyline points="9 22 9 12 15 12 15 22" />
                </svg>

                <span class="text-sm font-medium">"Dashboard"</span>
            </button>

            // Sub-items (visible when expanded)
            <Show when=move || dashboard_expanded.get()>
                <div class="ml-6 space-y-0.5">
                    <SidebarItem href="/" label="Overview">
                        <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                        <polyline points="9 22 9 12 15 12 15 22" />
                    </SidebarItem>
                    <SidebarItem href="/trace" label="Agent Trace" alert_key="agent.trace">
                        <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                    </SidebarItem>
                    <SidebarItem href="/status" label="System Health" alert_key="system.health">
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
                    <SidebarItem href="/memory" label="Memory Vault" alert_key="memory.status">
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    </SidebarItem>
                </div>
            </Show>
        </div>
    }
}

/// A settings group section with a label header and tab items
#[component]
fn SettingsGroupSection(
    label: &'static str,
    tabs: &'static [SettingsTab],
) -> impl IntoView {
    let location = use_location();

    view! {
        <div class="space-y-0.5">
            <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                {label}
            </h3>
            {tabs.iter().map(|tab| {
                let path = tab.path();
                let tab_label = tab.label();
                let icon_svg = tab.icon_svg();
                let is_active = move || location.pathname.get() == path;

                view! {
                    <A
                        href=path
                        attr:class=move || {
                            if is_active() {
                                "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 bg-sidebar-active text-sidebar-accent font-medium group"
                            } else {
                                "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 hover:bg-sidebar-active/50 group text-text-secondary hover:text-text-primary"
                            }
                        }
                    >
                        <svg
                            width="18"
                            height="18"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            class=move || {
                                if is_active() {
                                    "text-sidebar-accent flex-shrink-0"
                                } else {
                                    "text-text-tertiary group-hover:text-text-secondary flex-shrink-0"
                                }
                            }
                            inner_html=icon_svg
                        />
                        <span>{tab_label}</span>
                    </A>
                }
            }).collect_view()}
        </div>
    }
}

#[component]
fn ThemeToggle() -> impl IntoView {
    let initial = {
        let window = web_sys::window().unwrap();
        let storage: Option<web_sys::Storage> = window.local_storage().ok().flatten();
        match storage.as_ref().and_then(|s: &web_sys::Storage| s.get_item("aleph-theme").ok()).flatten().as_deref() {
            Some("light") => ThemeMode::Light,
            Some("dark") => ThemeMode::Dark,
            _ => ThemeMode::System,
        }
    };
    let (theme, set_theme) = signal(initial);

    let toggle = move |_| {
        let next = theme.get().next();
        set_theme.set(next);

        let window = web_sys::window().unwrap();
        let document = window.document().unwrap();
        let html = document.document_element().unwrap();
        let class_list = html.class_list();

        let _ = class_list.remove_2("dark", "light");

        let storage: Option<web_sys::Storage> = window.local_storage().ok().flatten();
        match next {
            ThemeMode::Light => {
                let _ = class_list.add_1("light");
                if let Some(s) = &storage { let _ = s.set_item("aleph-theme", "light"); }
            }
            ThemeMode::Dark => {
                let _ = class_list.add_1("dark");
                if let Some(s) = &storage { let _ = s.set_item("aleph-theme", "dark"); }
            }
            ThemeMode::System => {
                if let Some(s) = &storage { let _ = s.remove_item("aleph-theme"); }
            }
        }
    };

    let icon = move || match theme.get() {
        ThemeMode::System => view! {
            <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                <line x1="8" y1="21" x2="16" y2="21" />
                <line x1="12" y1="17" x2="12" y2="21" />
            </svg>
        }.into_any(),
        ThemeMode::Light => view! {
            <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="5" />
                <line x1="12" y1="1" x2="12" y2="3" />
                <line x1="12" y1="21" x2="12" y2="23" />
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
                <line x1="1" y1="12" x2="3" y2="12" />
                <line x1="21" y1="12" x2="23" y2="12" />
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
            </svg>
        }.into_any(),
        ThemeMode::Dark => view! {
            <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
            </svg>
        }.into_any(),
    };

    view! {
        <button
            on:click=toggle
            class="flex items-center gap-3 px-3 py-2 rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-all duration-200 w-full"
            title=move || format!("Theme: {}", theme.get().label())
        >
            {icon}
            <span class="text-sm font-medium">{move || theme.get().label()}</span>
        </button>
    }
}

#[component]
fn LogoSection() -> impl IntoView {
    view! {
        <div class="p-6 flex items-center gap-3">
            <div class="w-8 h-8 bg-primary rounded-lg flex items-center justify-center">
                <span class="text-text-inverse font-bold text-xl">"A"</span>
            </div>
            <h1 class="text-xl font-semibold tracking-tight">"Aleph Hub"</h1>
        </div>
    }
}
