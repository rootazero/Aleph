//! Native iPhone Agents screens. Mirrors the phone Chat/Memory drill-down
//! pattern: a list landing (`/agents`) — filter + new-agent form + agent cells —
//! drilling into a full-screen single-agent detail (`/agents/{id}/{tab}`) with a
//! horizontally-scrollable 5-tab bar (Overview/Files/Skills/Channels/Teams).
//! Reuses the agents data layer + the desktop tab content components (R4); only
//! the navigation chrome is phone-specific.

pub mod detail;
pub mod list;

use std::collections::HashMap;

use leptos::prelude::*;

use crate::api::agents::AgentSummary;

/// Router-owned state for the phone Agents screens. Every field is an
/// `RwSignal` (Copy), so the struct is `Copy` and travels via context.
#[derive(Clone, Copy)]
pub struct PhoneAgentsState {
    /// All agents (one `agents.list` window).
    pub agents: RwSignal<Vec<AgentSummary>>,
    /// agent_id → channel_name bindings (for the channel badge + filter).
    pub bindings: RwSignal<HashMap<String, String>>,
    pub loaded: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// Bumping this re-triggers the agents loader (Retry / after create / set_default / delete).
    pub reload_nonce: RwSignal<u32>,
}

/// The five per-agent detail tabs, mirroring the desktop `AgentsView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentDetailTab {
    Overview,
    Files,
    Skills,
    Channels,
    Teams,
}

impl AgentDetailTab {
    #[must_use]
    pub(crate) fn from_slug(s: &str) -> Self {
        match s {
            "files" => Self::Files,
            "skills" => Self::Skills,
            "channels" => Self::Channels,
            "teams" => Self::Teams,
            _ => Self::Overview,
        }
    }

    #[must_use]
    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Files => "files",
            Self::Skills => "skills",
            Self::Channels => "channels",
            Self::Teams => "teams",
        }
    }

    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Files => "Files",
            Self::Skills => "Skills",
            Self::Channels => "Channels",
            Self::Teams => "Teams",
        }
    }
}

pub(crate) const DETAIL_TABS: [AgentDetailTab; 5] = [
    AgentDetailTab::Overview,
    AgentDetailTab::Files,
    AgentDetailTab::Skills,
    AgentDetailTab::Channels,
    AgentDetailTab::Teams,
];

/// Parse `/agents/{id}` or `/agents/{id}/{tab}` → `(id, tab)`. Returns `None`
/// when no non-empty agent id is present (e.g. the bare `/agents` menu path).
#[must_use]
pub(crate) fn parse_detail_path(path: &str) -> Option<(String, AgentDetailTab)> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["agents", id, tab, ..] if !id.is_empty() => {
            Some(((*id).to_string(), AgentDetailTab::from_slug(tab)))
        }
        ["agents", id] if !id.is_empty() => Some(((*id).to_string(), AgentDetailTab::Overview)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_detail_path_extracts_id_and_tab() {
        assert_eq!(parse_detail_path("/agents"), None);
        assert_eq!(parse_detail_path("/agents/"), None);
        assert_eq!(
            parse_detail_path("/agents/zoe"),
            Some(("zoe".to_string(), AgentDetailTab::Overview))
        );
        assert_eq!(
            parse_detail_path("/agents/zoe/skills"),
            Some(("zoe".to_string(), AgentDetailTab::Skills))
        );
        assert_eq!(
            parse_detail_path("/agents/zoe/bogus"),
            Some(("zoe".to_string(), AgentDetailTab::Overview))
        );
    }
}
