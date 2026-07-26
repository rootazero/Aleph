//! The memory console's four fetches, each landing in a `Loadable` slot.
//!
//! Every one of these used to be an inline `Effect` doing `if let Ok(v) = ...`,
//! which turned a gateway error into an empty list. Routing them all through
//! `Loadable::from_rpc` means the error text survives to the renderer.
//!
//! Each function sets its slot to `Loading` before awaiting, so a slow refetch
//! shows skeletons rather than stale rows presented as current.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, MemoryApi, MemoryStats, RawMemory};
use crate::context::DashboardState;

/// One `list_facts` page plus the agent's total note count.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesWindow {
    pub facts: Vec<CompressedFact>,
    /// Total notes for this agent, independent of the window cap — lets the
    /// pager size itself and the truncation notice tell the truth.
    pub total: u64,
}

pub fn load_notes(
    state: DashboardState,
    agent: String,
    limit: usize,
    slot: RwSignal<Loadable<NotesWindow>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::list_facts(&state, &agent, limit, 0)
            .await
            .map(|(facts, total)| NotesWindow { facts, total });
        slot.set(Loadable::from_rpc(res));
    });
}

/// One `memory.search` page plus the count of rows matching the same filter.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWindow {
    pub raws: Vec<RawMemory>,
    /// Rows matching the active filter, independent of `limit`/`offset` — this
    /// is what lets the pager stop at the last page of a *filtered* result
    /// instead of sizing itself to the whole store.
    pub total: u64,
}

pub fn load_raw(
    state: DashboardState,
    agent: String,
    query: String,
    limit: u32,
    offset: u32,
    slot: RwSignal<Loadable<RawWindow>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::browse_raw(&state, &agent, query, limit, offset)
            .await
            .map(|(raws, total)| RawWindow { raws, total });
        slot.set(Loadable::from_rpc(res));
    });
}

/// Server-side note full-text search. Hits arrive as full index rows, so they
/// convert straight into the note card model with no follow-up round trip.
pub fn load_search_hits(
    state: DashboardState,
    agent: String,
    query: String,
    limit: usize,
    slot: RwSignal<Loadable<Vec<CompressedFact>>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = GraphApi::search(&state, &agent, &query, limit)
            .await
            .map(|r| {
                r.results
                    .iter()
                    .map(CompressedFact::from_search_hit)
                    .collect::<Vec<_>>()
            });
        slot.set(Loadable::from_rpc(res));
    });
}

pub fn load_stats(state: DashboardState, agent: String, slot: RwSignal<Loadable<MemoryStats>>) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::stats(&state, &agent).await;
        slot.set(Loadable::from_rpc(res));
    });
}
