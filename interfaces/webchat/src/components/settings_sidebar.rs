//! Settings tab definitions and group constants
//!
//! Provides `SettingsTab` enum and `SETTINGS_GROUPS` for sidebar navigation.
//! The sidebar component renders these directly (no separate `SettingsSidebar` component).

use crate::i18n::{t_string, Locale};
use leptos_i18n::I18nContext;

/// Settings tab identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    // Basic
    General,
    Appearance,
    Behavior,

    // AI
    Providers,
    EmbeddingProviders,
    RerankingProviders,
    GenerationProviders,
    ModelRoute,
    Moa,
    Memory,

    // Extensions
    Mcp,
    Plugins,
    Skills,
    Acp,

    // Channels
    Channels,

    // Advanced
    Browser,
    Search,
    Policies,
    RoutingRules,
    Security,
    Execution,

    // Network
    Network,
}

impl SettingsTab {
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::General => "/settings/general",
            Self::Appearance => "/settings/appearance",
            Self::Behavior => "/settings/behavior",
            Self::Providers => "/settings/providers",
            Self::EmbeddingProviders => "/settings/embedding-providers",
            Self::RerankingProviders => "/settings/reranking-providers",
            Self::GenerationProviders => "/settings/generation-providers",
            Self::ModelRoute => "/settings/model-route",
            Self::Moa => "/settings/moa",
            Self::Memory => "/settings/memory",
            Self::Mcp => "/settings/mcp",
            Self::Plugins => "/settings/plugins",
            Self::Skills => "/settings/skills",
            Self::Acp => "/settings/acp",
            Self::Channels => "/settings/channels",
            Self::Browser => "/settings/browser",
            Self::Search => "/settings/search",
            Self::Policies => "/settings/policies",
            Self::RoutingRules => "/settings/routing",
            Self::Security => "/settings/security",
            Self::Execution => "/settings/execution",
            Self::Network => "/settings/network",
        }
    }

    #[must_use]
    pub fn i18n_label(&self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::General => t_string!(i18n, settings.tabs.general).to_string(),
            Self::Appearance => t_string!(i18n, settings.tabs.appearance).to_string(),
            Self::Behavior => t_string!(i18n, settings.tabs.behavior).to_string(),
            Self::Providers => t_string!(i18n, settings.tabs.providers).to_string(),
            Self::EmbeddingProviders => t_string!(i18n, settings.tabs.embedding).to_string(),
            Self::RerankingProviders => t_string!(i18n, settings.tabs.reranking).to_string(),
            Self::GenerationProviders => t_string!(i18n, settings.tabs.generation).to_string(),
            Self::ModelRoute => t_string!(i18n, settings.tabs.model_route).to_string(),
            Self::Moa => t_string!(i18n, settings.tabs.moa).to_string(),
            Self::Memory => t_string!(i18n, settings.tabs.memory).to_string(),
            Self::Mcp => t_string!(i18n, settings.tabs.mcp).to_string(),
            Self::Plugins => t_string!(i18n, settings.tabs.plugins).to_string(),
            Self::Skills => t_string!(i18n, settings.tabs.skills).to_string(),
            Self::Acp => t_string!(i18n, settings.tabs.acp).to_string(),
            Self::Channels => t_string!(i18n, settings.tabs.channels).to_string(),
            Self::Browser => t_string!(i18n, settings.tabs.browser).to_string(),
            Self::Search => t_string!(i18n, settings.tabs.search).to_string(),
            Self::Policies => t_string!(i18n, settings.tabs.policies).to_string(),
            Self::RoutingRules => t_string!(i18n, settings.tabs.routing_rules).to_string(),
            Self::Security => t_string!(i18n, settings.tabs.security).to_string(),
            Self::Execution => t_string!(i18n, settings.tabs.execution).to_string(),
            Self::Network => t_string!(i18n, settings.tabs.network).to_string(),
        }
    }

    #[must_use]
    pub const fn icon_svg(&self) -> &'static str {
        match self {
            Self::General => {
                r#"<circle cx="12" cy="12" r="3"/><path d="M12 1v6m0 6v6M5.64 5.64l4.24 4.24m4.24 4.24l4.24 4.24M1 12h6m6 0h6M5.64 18.36l4.24-4.24m4.24-4.24l4.24-4.24"/>"#
            }
            Self::Appearance => {
                r#"<circle cx="13.5" cy="6.5" r=".5" fill="currentColor"/><circle cx="17.5" cy="10.5" r=".5" fill="currentColor"/><circle cx="8.5" cy="7.5" r=".5" fill="currentColor"/><circle cx="6.5" cy="12.5" r=".5" fill="currentColor"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"/>"#
            }
            Self::Behavior => {
                r#"<path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2z"/><path d="M12 6v6l4 2"/>"#
            }
            Self::Providers => {
                r#"<path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/>"#
            }
            Self::EmbeddingProviders => {
                r#"<circle cx="12" cy="12" r="2"/><circle cx="6" cy="6" r="2"/><circle cx="18" cy="6" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="18" r="2"/><line x1="12" y1="10" x2="12" y2="14"/><line x1="7.5" y1="7.5" x2="10.5" y2="10.5"/><line x1="13.5" y1="10.5" x2="16.5" y2="7.5"/><line x1="7.5" y1="16.5" x2="10.5" y2="13.5"/><line x1="13.5" y1="13.5" x2="16.5" y2="16.5"/>"#
            }
            Self::RerankingProviders => {
                r#"<path d="M3 6h18M3 12h18M3 18h18"/><path d="M7 3v3M7 15v3M12 9v3M17 3v3M17 15v3"/>"#
            }
            Self::GenerationProviders => {
                r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>"#
            }
            Self::ModelRoute => {
                r#"<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="12" r="3"/><path d="M9 6h3a3 3 0 0 1 3 3v0M9 18h3a3 3 0 0 0 3-3v0"/>"#
            }
            Self::Moa => {
                r#"<circle cx="7" cy="7" r="3"/><circle cx="17" cy="7" r="3"/><circle cx="12" cy="18" r="3"/><path d="M9 9l3 6.5M15 9l-3 6.5"/>"#
            }
            Self::Memory => {
                r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>"#
            }
            Self::Mcp => {
                r#"<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>"#
            }
            Self::Plugins => r#"<circle cx="12" cy="12" r="3"/><path d="M12 1v6m0 6v6"/>"#,
            Self::Skills => {
                r#"<path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"/>"#
            }
            Self::Acp => {
                r#"<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>"#
            }
            Self::Channels => {
                r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#
            }
            Self::Browser => {
                r#"<circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>"#
            }
            Self::Search => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/>"#,
            Self::Policies => {
                r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
            }
            Self::RoutingRules => {
                r#"<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>"#
            }
            Self::Security => {
                r#"<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
            }
            Self::Execution => r#"<path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"/>"#,
            Self::Network => {
                r#"<circle cx="5" cy="6" r="2"/><circle cx="5" cy="18" r="2"/><circle cx="19" cy="12" r="2"/><path d="M7 6h6a3 3 0 0 1 3 3v0M7 18h6a3 3 0 0 0 3-3v0"/>"#
            }
        }
    }
}

/// Settings group definition
pub struct SettingsGroup {
    pub label: &'static str,
    pub tabs: &'static [SettingsTab],
}

impl SettingsGroup {
    #[must_use]
    pub fn i18n_label(&self, i18n: I18nContext<Locale>) -> String {
        match self.label {
            "Basic" => t_string!(i18n, settings.groups.basic).to_string(),
            "AI" => t_string!(i18n, settings.groups.ai).to_string(),
            "Channels" => t_string!(i18n, settings.groups.channels).to_string(),
            "Extensions" => t_string!(i18n, settings.groups.extensions).to_string(),
            "Advanced" => t_string!(i18n, settings.groups.advanced).to_string(),
            "Network" => "服务与集群".to_string(),
            other => other.to_string(),
        }
    }
}

pub const SETTINGS_GROUPS: &[SettingsGroup] = &[
    SettingsGroup {
        label: "Basic",
        tabs: &[
            SettingsTab::General,
            SettingsTab::Appearance,
            SettingsTab::Behavior,
        ],
    },
    SettingsGroup {
        label: "AI",
        tabs: &[
            SettingsTab::Providers,
            SettingsTab::EmbeddingProviders,
            SettingsTab::RerankingProviders,
            SettingsTab::GenerationProviders,
            SettingsTab::ModelRoute,
            SettingsTab::Moa,
            SettingsTab::RoutingRules,
            SettingsTab::Search,
            SettingsTab::Memory,
        ],
    },
    SettingsGroup {
        label: "Channels",
        tabs: &[SettingsTab::Channels],
    },
    SettingsGroup {
        label: "Extensions",
        tabs: &[
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
            SettingsTab::Acp,
        ],
    },
    SettingsGroup {
        label: "Advanced",
        tabs: &[
            SettingsTab::Browser,
            SettingsTab::Policies,
            SettingsTab::Security,
            SettingsTab::Execution,
        ],
    },
    SettingsGroup {
        label: "Network",
        tabs: &[SettingsTab::Network],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tab path reachable from the sidebar navigation.
    fn all_tab_paths() -> Vec<&'static str> {
        SETTINGS_GROUPS
            .iter()
            .flat_map(|g| g.tabs.iter().map(|t| t.path()))
            .collect()
    }

    #[test]
    fn clawhub_tab_is_removed() {
        // ClawHub is subsumed into the Extensions store (P5); its settings tab must be gone.
        assert!(
            !all_tab_paths().contains(&"/settings/clawhub"),
            "ClawHub settings tab must be fully removed from SETTINGS_GROUPS"
        );
    }

    /// Tab paths in the named settings group.
    fn group_tab_paths(label: &str) -> Vec<&'static str> {
        SETTINGS_GROUPS
            .iter()
            .find(|g| g.label == label)
            .map(|g| g.tabs.iter().map(|t| t.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn mcp_plugins_skills_in_extensions_group() {
        let extensions = group_tab_paths("Extensions");
        assert!(
            extensions.contains(&"/settings/mcp"),
            "Extensions must contain MCP"
        );
        assert!(
            extensions.contains(&"/settings/plugins"),
            "Extensions must contain Plugins"
        );
        assert!(
            extensions.contains(&"/settings/skills"),
            "Extensions must contain Skills"
        );

        let advanced = group_tab_paths("Advanced");
        assert!(
            !advanced.contains(&"/settings/mcp"),
            "Advanced must not contain MCP"
        );
        assert!(
            !advanced.contains(&"/settings/plugins"),
            "Advanced must not contain Plugins"
        );
        assert!(
            !advanced.contains(&"/settings/skills"),
            "Advanced must not contain Skills"
        );
    }
}
