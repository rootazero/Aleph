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
    /// pager size itself and the truncation notice tell the truth. `None`
    /// when an un-upgraded core didn't report one at all — genuinely
    /// unknown, not zero.
    pub total: Option<u64>,
}

pub fn load_notes(
    state: DashboardState,
    agent: String,
    limit: usize,
    slot: RwSignal<Loadable<NotesWindow>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::list_facts(&state, &agent, limit, 0).await;
        slot.set(Loadable::from_rpc(to_notes_window(res)));
    });
}

/// Pure mapping step of [`load_notes`], split out so `total`'s source is
/// unit-testable without a transport mock: it must come from the RPC's
/// second tuple element (the agent's full count), never from `facts.len()`.
fn to_notes_window(
    res: Result<(Vec<CompressedFact>, Option<u64>), String>,
) -> Result<NotesWindow, String> {
    res.map(|(facts, total)| NotesWindow { facts, total })
}

/// One `memory.search` page plus the count of rows matching the same filter.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWindow {
    pub raws: Vec<RawMemory>,
    /// Rows matching the active filter, independent of `limit`/`offset` — this
    /// is what lets the pager stop at the last page of a *filtered* result
    /// instead of sizing itself to the whole store. `None` when an
    /// un-upgraded core didn't report one at all, which the pager reads as
    /// genuinely unknown rather than "no more rows".
    pub total: Option<u64>,
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
        let res = MemoryApi::browse_raw(&state, &agent, query, limit, offset).await;
        slot.set(Loadable::from_rpc(to_raw_window(res)));
    });
}

/// Pure mapping step of [`load_raw`], split out so `total`'s source is
/// unit-testable without a transport mock: it must come from the RPC's
/// second tuple element (rows matching the active filter), never from
/// `raws.len()` — conflating the two is exactly what revives the phantom
/// trailing page under an active query.
fn to_raw_window(res: Result<(Vec<RawMemory>, Option<u64>), String>) -> Result<RawWindow, String> {
    res.map(|(raws, total)| RawWindow { raws, total })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(path: &str) -> CompressedFact {
        CompressedFact {
            id: path.into(),
            agent_id: "main".into(),
            content: "c".into(),
            fact_type: "preference".into(),
            created_at: 0,
            updated_at: 0,
            category: "preference".into(),
            path: path.into(),
            tags: Vec::new(),
            link_count: 0,
        }
    }

    fn raw(id: &str) -> RawMemory {
        RawMemory {
            id: id.into(),
            agent_id: "main".into(),
            user_input: "q".into(),
            ai_output: "a".into(),
            session_id: None,
            created_at: None,
        }
    }

    // ── to_notes_window ─────────────────────────────────────────────────────

    #[test]
    fn notes_window_total_comes_from_the_tuple_not_row_count() {
        // 2 rows but a total of 7: a `facts.len() as u64` implementation
        // would report 2 here, so this pins the real contract.
        let res = Ok((vec![fact("a"), fact("b")], Some(7u64)));
        let window = to_notes_window(res).expect("Ok input stays Ok");
        assert_eq!(window.total, Some(7));
        assert_eq!(window.facts.len(), 2);
    }

    #[test]
    fn notes_window_total_is_none_when_the_core_never_sent_it() {
        // Version skew: an un-upgraded core omits the field entirely. `None`
        // must survive here, not fold into `0` (which the truncation notice
        // would read as "the store is empty").
        let res = Ok((vec![fact("a")], None));
        let window = to_notes_window(res).expect("Ok input stays Ok");
        assert_eq!(window.total, None);
    }

    #[test]
    fn notes_window_err_stays_err_with_its_message() {
        let res: Result<(Vec<CompressedFact>, Option<u64>), String> = Err("gateway timeout".into());
        assert_eq!(
            to_notes_window(res),
            Err("gateway timeout".to_string()),
            "a failure must not fold into an empty window"
        );
    }

    // ── to_raw_window ────────────────────────────────────────────────────────

    #[test]
    fn raw_window_total_comes_from_the_tuple_not_row_count() {
        // 3 rows but a total of 41 (the filtered count from a much larger
        // match set): a `raws.len() as u64` implementation would report 3
        // here, which is exactly the phantom-page bug this window exists to
        // prevent.
        let res = Ok((vec![raw("r1"), raw("r2"), raw("r3")], Some(41u64)));
        let window = to_raw_window(res).expect("Ok input stays Ok");
        assert_eq!(window.total, Some(41));
        assert_eq!(window.raws.len(), 3);
    }

    #[test]
    fn raw_window_total_is_none_when_the_core_never_sent_it() {
        // Version skew: `None` (unknown), not `0` (which the pager would
        // read as "no more rows" and hide the next-page control entirely).
        let res = Ok((vec![raw("r1")], None));
        let window = to_raw_window(res).expect("Ok input stays Ok");
        assert_eq!(window.total, None);
    }

    #[test]
    fn raw_window_err_stays_err_with_its_message() {
        let res: Result<(Vec<RawMemory>, Option<u64>), String> = Err("gateway timeout".into());
        assert_eq!(
            to_raw_window(res),
            Err("gateway timeout".to_string()),
            "a failure must not fold into an empty window"
        );
    }
}
