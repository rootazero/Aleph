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
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agent_binding::AgentBindingApi;
use crate::api::agents::{AgentSummary, AgentsApi};
use crate::context::DashboardState;

use self::detail::PhoneAgentDetail;
use self::list::PhoneAgentsList;

/// Router-owned state for the phone Agents screens. Every field is an
/// `RwSignal` (Copy), so the struct is `Copy` and travels via context.
#[derive(Clone, Copy)]
pub struct PhoneAgentsState {
    /// All agents (one `agents.list` window).
    pub agents: RwSignal<Vec<AgentSummary>>,
    /// agent_id → bound channels (for the channel badge + filter).
    /// Many-to-one: an agent may be bound to several channels.
    pub bindings: RwSignal<HashMap<String, Vec<String>>>,
    pub loaded: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// Bumping this re-triggers the agents loader (Retry / after create / set_default / delete).
    pub reload_nonce: RwSignal<u32>,
}

/// Phone Agents router. Owns `PhoneAgentsState`, connect-gated-loads the agent
/// list + bindings, and renders the list at `/agents` or the detail at
/// `/agents/{id}/…`. Request/response only (no streaming subscription).
#[component]
#[must_use]
pub fn PhoneAgents() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();

    let st = PhoneAgentsState {
        agents: RwSignal::new(Vec::new()),
        bindings: RwSignal::new(HashMap::new()),
        loaded: RwSignal::new(false),
        error: RwSignal::new(None),
        reload_nonce: RwSignal::new(0),
    };
    provide_context(st);

    // Agents loader — connect-gated (cold-boot lesson). Re-runs when
    // `reload_nonce` is bumped (Retry, or after create/set_default/delete).
    Effect::new(move || {
        st.reload_nonce.get();
        if dashboard.is_connected.get() {
            spawn_local(async move {
                st.loaded.set(false);
                st.error.set(None);
                match AgentsApi::list(&dashboard).await {
                    Ok(resp) => {
                        st.agents.set(resp.agents);
                        // Bindings are best-effort: a failure leaves the badge/
                        // filter empty but never blocks the list.
                        if let Ok(map) = AgentBindingApi::agent_bindings(&dashboard).await {
                            st.bindings.set(map);
                        }
                    }
                    Err(e) => st.error.set(Some(e)),
                }
                st.loaded.set(true);
            });
        } else {
            st.agents.set(Vec::new());
            st.loaded.set(false);
        }
    });

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        AgentsScreen::Detail => view! { <PhoneAgentDetail/> }.into_any(),
        AgentsScreen::Menu => view! { <PhoneAgentsList/> }.into_any(),
    }
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

/// Which phone Agents screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsScreen {
    Menu,
    Detail,
}

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> AgentsScreen {
    if path == "/agents" || path == "/agents/" {
        AgentsScreen::Menu
    } else {
        AgentsScreen::Detail
    }
}

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

    #[test]
    fn screen_for_path_menu_only_for_bare_agents() {
        assert_eq!(screen_for_path("/agents"), AgentsScreen::Menu);
        assert_eq!(screen_for_path("/agents/"), AgentsScreen::Menu);
        assert_eq!(screen_for_path("/agents/abc"), AgentsScreen::Detail);
        assert_eq!(screen_for_path("/agents/abc/files"), AgentsScreen::Detail);
    }
}
