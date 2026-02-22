//! Settings sidebar component with grouped navigation
//!
//! This module provides a sidebar navigation for settings pages,
//! organized into logical groups similar to macOS System Settings.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

/// Settings tab identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    // Basic
    General,
    Shortcuts,
    Behavior,

    // AI
    Providers,
    GenerationProviders,
    Memory,

    // Extensions
    Mcp,
    Plugins,
    Skills,

    // Channels
    Discord,

    // Advanced
    Agent,
    Search,
    Policies,
    RoutingRules,
    Security,
}

impl SettingsTab {
    pub fn path(&self) -> &'static str {
        match self {
            Self::General => "/settings/general",
            Self::Shortcuts => "/settings/shortcuts",
            Self::Behavior => "/settings/behavior",
            Self::Providers => "/settings/providers",
            Self::GenerationProviders => "/settings/generation-providers",
            Self::Memory => "/settings/memory",
            Self::Mcp => "/settings/mcp",
            Self::Plugins => "/settings/plugins",
            Self::Skills => "/settings/skills",
            Self::Discord => "/settings/channels/discord",
            Self::Agent => "/settings/agent",
            Self::Search => "/settings/search",
            Self::Policies => "/settings/policies",
            Self::RoutingRules => "/settings/routing",
            Self::Security => "/settings/security",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Shortcuts => "Shortcuts",
            Self::Behavior => "Behavior",
            Self::Providers => "AI Providers",
            Self::GenerationProviders => "Generation Providers",
            Self::Memory => "Memory & Knowledge",
            Self::Mcp => "MCP Plugins",
            Self::Plugins => "Plugins",
            Self::Skills => "Skills",
            Self::Discord => "Discord",
            Self::Agent => "Agent Behavior",
            Self::Search => "Search",
            Self::Policies => "Policies",
            Self::RoutingRules => "Routing Rules",
            Self::Security => "Security",
        }
    }

    pub fn icon_svg(&self) -> &'static str {
        match self {
            Self::General => r#"<circle cx="12" cy="12" r="3"/><path d="M12 1v6m0 6v6M5.64 5.64l4.24 4.24m4.24 4.24l4.24 4.24M1 12h6m6 0h6M5.64 18.36l4.24-4.24m4.24-4.24l4.24-4.24"/>"#,
            Self::Shortcuts => r#"<rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/>"#,
            Self::Behavior => r#"<path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2z"/><path d="M12 6v6l4 2"/>"#,
            Self::Providers => r#"<path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/>"#,
            Self::GenerationProviders => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>"#,
            Self::Memory => r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>"#,
            Self::Mcp => r#"<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>"#,
            Self::Plugins => r#"<circle cx="12" cy="12" r="3"/><path d="M12 1v6m0 6v6"/>"#,
            Self::Skills => r#"<path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"/>"#,
            Self::Discord => r#"<path d="M18.59 5.89c-1.23-.57-2.54-.99-3.92-1.23-.17.3-.37.71-.5 1.03-1.46-.22-2.91-.22-4.34 0-.14-.32-.34-.73-.51-1.03-1.38.24-2.69.66-3.92 1.23C2.18 10.73 1.34 15.44 1.76 20.09A18.07 18.07 0 0 0 7.2 22.5c.44-.6.83-1.24 1.17-1.91-.64-.24-1.26-.54-1.84-.89.15-.11.3-.23.45-.34a12.84 12.84 0 0 0 10.04 0c.15.12.3.23.45.34-.58.35-1.2.65-1.84.89.34.67.73 1.31 1.17 1.91a18 18 0 0 0 5.44-2.41c.49-5.15-.84-9.82-3.65-13.61zM8.35 17.24c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42zm6.3 0c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42z"/>"#,
            Self::Agent => r#"<path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2z"/><path d="M12 6v6l4 2"/>"#,
            Self::Search => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/>"#,
            Self::Policies => r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
            Self::RoutingRules => r#"<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>"#,
            Self::Security => r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
        }
    }
}

/// Settings group definition
struct SettingsGroup {
    label: &'static str,
    tabs: &'static [SettingsTab],
}

const SETTINGS_GROUPS: &[SettingsGroup] = &[
    SettingsGroup {
        label: "Basic",
        tabs: &[
            SettingsTab::General,
            SettingsTab::Shortcuts,
            SettingsTab::Behavior,
        ],
    },
    SettingsGroup {
        label: "AI",
        tabs: &[
            SettingsTab::Providers,
            SettingsTab::GenerationProviders,
            SettingsTab::Memory,
        ],
    },
    SettingsGroup {
        label: "Channels",
        tabs: &[
            SettingsTab::Discord,
        ],
    },
    SettingsGroup {
        label: "Extensions",
        tabs: &[
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
        ],
    },
    SettingsGroup {
        label: "Advanced",
        tabs: &[
            SettingsTab::Agent,
            SettingsTab::Search,
            SettingsTab::Policies,
            SettingsTab::RoutingRules,
            SettingsTab::Security,
        ],
    },
];

/// Settings sidebar component
#[component]
pub fn SettingsSidebar() -> impl IntoView {
    view! {
        <nav class="w-64 bg-sidebar border-r border-border p-4 space-y-6 overflow-y-auto">
            <div class="mb-6">
                <h2 class="text-xl font-bold text-text-primary">
                    "Settings"
                </h2>
                <p class="text-xs text-text-tertiary mt-1">
                    "Configure Aleph Gateway"
                </p>
            </div>

            {SETTINGS_GROUPS.iter().map(|group| {
                view! {
                    <div class="space-y-1">
                        <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group.label}
                        </h3>
                        <div class="space-y-0.5">
                            {group.tabs.iter().map(|tab| {
                                view! {
                                    <SettingsSidebarItem tab=*tab />
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }
            }).collect_view()}
        </nav>
    }
}

/// Individual sidebar item
#[component]
fn SettingsSidebarItem(tab: SettingsTab) -> impl IntoView {
    let path = tab.path();
    let label = tab.label();
    let icon_svg = tab.icon_svg();
    let location = use_location();

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
            exact=true
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
                        "text-sidebar-accent"
                    } else {
                        "text-text-tertiary group-hover:text-text-secondary"
                    }
                }
                inner_html=icon_svg
            />
            <span>
                {label}
            </span>
        </A>
    }
}
