//! Shared memory-mode state, lifted from `RadialCanvasView` so the
//! sidebar (search / fold / detail panel) and the canvas itself can
//! both read and mutate it.

use crate::api::agents::AgentSummary;
use leptos::prelude::*;
use std::collections::VecDeque;

/// Cap on how many nodes are retained in `recent_visited`.
const RECENT_VISITED_CAPACITY: usize = 8;

#[derive(Clone, Copy)]
pub struct MemoryState {
    pub agent_id: RwSignal<String>,
    pub agents: RwSignal<Vec<AgentSummary>>,
    pub search_query: RwSignal<String>,
    pub fold_threshold: RwSignal<usize>,
    pub selected_node: RwSignal<Option<String>>,
    pub focus_id: RwSignal<Option<String>>,
    pub breadcrumb_entries: RwSignal<Vec<String>>,
    pub recent_visited: RwSignal<VecDeque<String>>,
    pub sidebar_collapsed: RwSignal<bool>,
}

impl MemoryState {
    #[must_use]
    pub fn new() -> Self {
        // Read persisted collapsed state from localStorage (default: false).
        let initial_collapsed = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item("aleph.sidebar.collapsed").ok().flatten())
            .map(|v| v == "1")
            .unwrap_or(false);
        let sidebar_collapsed = RwSignal::new(initial_collapsed);

        // Persist sidebar_collapsed to localStorage whenever it changes.
        // This Effect runs inside a Leptos component (AppContent), so a
        // reactive owner is always present.
        Effect::new(move |_| {
            let v = sidebar_collapsed.get();
            if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
                let _ = s.set_item("aleph.sidebar.collapsed", if v { "1" } else { "0" });
            }
        });

        Self {
            agent_id: RwSignal::new("main".into()),
            agents: RwSignal::new(Vec::new()),
            search_query: RwSignal::new(String::new()),
            fold_threshold: RwSignal::new(3),
            selected_node: RwSignal::new(None),
            focus_id: RwSignal::new(None),
            breadcrumb_entries: RwSignal::new(Vec::new()),
            recent_visited: RwSignal::new(VecDeque::with_capacity(RECENT_VISITED_CAPACITY)),
            sidebar_collapsed,
        }
    }

    /// Push `id` to the front of `recent_visited`, dropping oldest beyond capacity 8.
    /// De-duplicates: existing copies are removed first so the most-recent entry
    /// always sits at the front.
    pub fn push_recent(&self, id: String) {
        self.recent_visited.update(|q| {
            push_recent_into(q, id);
        });
    }
}

impl Default for MemoryState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Pure helper — tested independently of the Leptos reactive runtime.
// `MemoryState::push_recent` delegates here so the same logic is exercised
// by both production code and the unit tests below.
// ---------------------------------------------------------------------------

pub(crate) fn push_recent_into(q: &mut VecDeque<String>, id: String) {
    q.retain(|x| x != &id);
    q.push_front(id);
    while q.len() > RECENT_VISITED_CAPACITY {
        q.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_recent_caps_at_8_and_dedupes() {
        let mut q: VecDeque<String> = VecDeque::new();

        for i in 0..12 {
            push_recent_into(&mut q, format!("id-{i}"));
        }
        assert_eq!(q.len(), 8);
        assert_eq!(q[0], "id-11"); // most recent at front

        // Re-push an existing id — must de-dupe and float to front.
        push_recent_into(&mut q, "id-5".into());
        assert_eq!(q[0], "id-5");
        assert_eq!(q.iter().filter(|x| **x == "id-5").count(), 1);
        // Length must not exceed cap.
        assert_eq!(q.len(), 8);
    }

    #[test]
    fn push_recent_empty_then_single() {
        let mut q: VecDeque<String> = VecDeque::new();
        push_recent_into(&mut q, "alpha".into());
        assert_eq!(q.len(), 1);
        assert_eq!(q[0], "alpha");
    }

    #[test]
    fn push_recent_same_id_repeatedly_stays_length_1() {
        let mut q: VecDeque<String> = VecDeque::new();
        for _ in 0..5 {
            push_recent_into(&mut q, "x".into());
        }
        assert_eq!(q.len(), 1);
        assert_eq!(q[0], "x");
    }
}
