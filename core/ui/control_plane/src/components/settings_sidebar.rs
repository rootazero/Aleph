//! Settings tab definitions and group constants
//!
//! Provides `SettingsTab` enum and `SETTINGS_GROUPS` for sidebar navigation.
//! The sidebar component renders these directly (no separate SettingsSidebar component).

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
    Telegram,
    Discord,
    WhatsApp,
    IMessage,

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
            Self::Telegram => "/settings/channels/telegram",
            Self::Discord => "/settings/channels/discord",
            Self::WhatsApp => "/settings/channels/whatsapp",
            Self::IMessage => "/settings/channels/imessage",
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
            Self::Telegram => "Telegram",
            Self::Discord => "Discord",
            Self::WhatsApp => "WhatsApp",
            Self::IMessage => "iMessage",
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
            Self::Telegram => r#"<path d="M21.2 4.4L2.9 11.3c-1.2.5-1.2 1.2-.2 1.5l4.7 1.5 1.8 5.6c.2.6.1.8.7.8.4 0 .6-.2.9-.4l2.1-2.1 4.4 3.3c.8.4 1.4.2 1.6-.8L22.4 5.6c.3-1.2-.5-1.7-1.2-1.2zM8.5 13.5l9.4-5.9c.4-.3.8-.1.5.2l-7.8 7-.3 3.2-1.8-4.5z"/>"#,
            Self::Discord => r#"<path d="M18.59 5.89c-1.23-.57-2.54-.99-3.92-1.23-.17.3-.37.71-.5 1.03-1.46-.22-2.91-.22-4.34 0-.14-.32-.34-.73-.51-1.03-1.38.24-2.69.66-3.92 1.23C2.18 10.73 1.34 15.44 1.76 20.09A18.07 18.07 0 0 0 7.2 22.5c.44-.6.83-1.24 1.17-1.91-.64-.24-1.26-.54-1.84-.89.15-.11.3-.23.45-.34a12.84 12.84 0 0 0 10.04 0c.15.12.3.23.45.34-.58.35-1.2.65-1.84.89.34.67.73 1.31 1.17 1.91a18 18 0 0 0 5.44-2.41c.49-5.15-.84-9.82-3.65-13.61zM8.35 17.24c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42zm6.3 0c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42z"/>"#,
            Self::WhatsApp => r#"<path d="M17.47 14.38c-.29-.14-1.7-.84-1.96-.94-.27-.1-.46-.14-.65.14-.2.29-.75.94-.92 1.13-.17.2-.34.22-.63.07-.29-.14-1.22-.45-2.32-1.43-.86-.77-1.44-1.71-1.61-2-.17-.29-.02-.45.13-.59.13-.13.29-.34.44-.51.14-.17.2-.29.29-.48.1-.2.05-.37-.02-.51-.07-.15-.65-1.56-.89-2.14-.24-.56-.48-.49-.65-.49-.17 0-.37-.02-.56-.02-.2 0-.51.07-.78.37-.27.29-1.02 1-1.02 2.43 0 1.43 1.04 2.82 1.19 3.01.14.2 2.05 3.13 4.97 4.39.7.3 1.24.48 1.66.61.7.22 1.33.19 1.83.12.56-.08 1.7-.7 1.94-1.37.24-.68.24-1.26.17-1.38-.07-.12-.27-.2-.56-.34zM12 2C6.48 2 2 6.48 2 12c0 1.77.46 3.43 1.27 4.88L2 22l5.23-1.37A9.93 9.93 0 0 0 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>"#,
            Self::IMessage => r#"<path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>"#,
            Self::Agent => r#"<path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2z"/><path d="M12 6v6l4 2"/>"#,
            Self::Search => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/>"#,
            Self::Policies => r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
            Self::RoutingRules => r#"<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>"#,
            Self::Security => r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
        }
    }
}

/// Settings group definition
pub struct SettingsGroup {
    pub label: &'static str,
    pub tabs: &'static [SettingsTab],
}

pub const SETTINGS_GROUPS: &[SettingsGroup] = &[
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
            SettingsTab::Telegram,
            SettingsTab::Discord,
            SettingsTab::WhatsApp,
            SettingsTab::IMessage,
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
