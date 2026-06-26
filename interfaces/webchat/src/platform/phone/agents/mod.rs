//! Native iPhone Agents screens. Mirrors the phone Chat/Memory drill-down
//! pattern: a list landing (`/agents`) — filter + new-agent form + agent cells —
//! drilling into a full-screen single-agent detail (`/agents/{id}/{tab}`) with a
//! horizontally-scrollable 5-tab bar (Overview/Files/Skills/Channels/Teams).
//! Reuses the agents data layer + the desktop tab content components (R4); only
//! the navigation chrome is phone-specific.

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
