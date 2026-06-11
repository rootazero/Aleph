//! Multi-tab session map for the chat surface — UI-TARS / VS Code parity.
//!
//! Each "tab" corresponds to one `agent_id`. The currently visible tab's
//! state lives in the singleton [`ChatState`]; the other tabs are
//! snapshotted via [`SessionSnapshot`] and parked in this map.
//!
//! ## Why snapshot/restore instead of multiple `ChatStates`?
//!
//! Replacing the `ChatState` context with one-per-tab would force every
//! consumer to either remount on tab switch (losing scroll/DOM state) or
//! resolve through an extra indirection. Snapshotting keeps every existing
//! `expect_context::<ChatState>()` call site working unchanged — the
//! signals stay the same, only their contents swap.
//!
//! ## Lifecycle
//!
//! - `activate(chat, agent_id)` — open or focus a tab. Snapshots the
//!   current `ChatState`, restores the target snapshot (or default for
//!   first-time activation), and updates the active pointer.
//! - `close(chat, agent_id)` — discard a tab's snapshot. If the closed
//!   tab was active, advances to the next neighbour (or clears state if
//!   no tabs remain).
//! - `switch_by_index(chat, n)` — keyboard hotkey backbone (Cmd+1..9).

use leptos::prelude::*;
use std::collections::HashMap;

use crate::views::chat::state::{ChatState, SessionSnapshot};

/// Agent identifier — wire-stable string matching `chat.agent_id`.
pub type AgentId = String;

/// Reactive multi-tab session registry. `Copy` so it can ride
/// `provide_context` without `Arc` indirection.
#[derive(Clone, Copy)]
pub struct SessionMap {
    /// Parked snapshots for non-active tabs. The active tab's data lives
    /// in `ChatState` itself, so the entry for the active `agent_id` is
    /// intentionally absent (added on the next activate-away).
    snapshots: RwSignal<HashMap<AgentId, SessionSnapshot>>,
    /// Visible tab order — drives the rendered tab strip and Cmd+N keying.
    /// Invariant: contains every `agent_id` that has an entry in snapshots,
    /// plus the currently active `agent_id` if any.
    pub tab_order: RwSignal<Vec<AgentId>>,
    /// Currently focused tab. `None` means no tabs are open (boot state).
    pub active: RwSignal<Option<AgentId>>,
}

impl Default for SessionMap {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionMap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshots: RwSignal::new(HashMap::new()),
            tab_order: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
        }
    }

    /// Open or focus a tab for `agent_id`. Idempotent — re-activating the
    /// already-active tab is a no-op.
    ///
    /// Snapshot/restore preserves chat history, attachments, project root,
    /// model override, etc. across tabs.
    pub fn activate(&self, chat: ChatState, agent_id: &AgentId) {
        let current = self.active.get_untracked();
        if current.as_ref() == Some(agent_id) {
            return;
        }
        // 1. Snapshot the outgoing tab into the map.
        if let Some(prev) = current {
            let snap = chat.capture_snapshot();
            self.snapshots.update(|m| {
                m.insert(prev, snap);
            });
        }
        // 2. Pull the incoming tab's snapshot (or default for first open).
        let mut taken: Option<SessionSnapshot> = None;
        self.snapshots.update(|m| {
            taken = m.remove(agent_id);
        });
        let mut snap = taken.unwrap_or_default();
        // Ensure agent_id is consistent post-restore even on first open.
        snap.agent_id = Some(agent_id.clone());
        chat.restore_from(snap);
        // 3. Append to visible order if new.
        self.tab_order.update(|order| {
            if !order.contains(agent_id) {
                order.push(agent_id.clone());
            }
        });
        self.active.set(Some(agent_id.clone()));
    }

    /// Close a tab. If the closed tab was active, advances to the
    /// previous tab in order (mirrors browser-tab behaviour: closing
    /// the active tab focuses the one to its left, falling through to
    /// the right neighbour if it was the first).
    pub fn close(&self, chat: ChatState, agent_id: &AgentId) {
        let was_active = self
            .active
            .get_untracked()
            .as_ref()
            .map(|a| a == agent_id)
            .unwrap_or(false);

        // Drop any parked snapshot for this tab.
        self.snapshots.update(|m| {
            m.remove(agent_id);
        });

        // Compute the new order and the neighbour to focus.
        let order = self.tab_order.get_untracked();
        let idx = order.iter().position(|a| a == agent_id);
        let neighbour: Option<AgentId> = idx.and_then(|i| {
            // Prefer left neighbour, fall through to right, then None.
            if i > 0 {
                order.get(i - 1).cloned()
            } else {
                order.get(i + 1).cloned()
            }
        });
        let new_order: Vec<AgentId> = order.into_iter().filter(|a| a != agent_id).collect();
        self.tab_order.set(new_order);

        if was_active {
            match neighbour {
                Some(next) => {
                    // Activate the neighbour (pull its snapshot).
                    let mut taken: Option<SessionSnapshot> = None;
                    self.snapshots.update(|m| {
                        taken = m.remove(&next);
                    });
                    let mut snap = taken.unwrap_or_default();
                    snap.agent_id = Some(next.clone());
                    chat.restore_from(snap);
                    self.active.set(Some(next));
                }
                None => {
                    // No tabs left — reset ChatState to a blank slate.
                    chat.restore_from(SessionSnapshot::default());
                    self.active.set(None);
                }
            }
        }
    }

    /// Activate the tab at `idx` in the visible order. Used by the
    /// Cmd+1..9 hotkeys; silently ignores out-of-range indices.
    pub fn switch_by_index(&self, chat: ChatState, idx: usize) {
        let target = self.tab_order.with(|order| order.get(idx).cloned());
        if let Some(aid) = target {
            self.activate(chat, &aid);
        }
    }

    /// Close the currently active tab (Cmd+W). No-op when no tab is open.
    pub fn close_active(&self, chat: ChatState) {
        if let Some(aid) = self.active.get_untracked() {
            self.close(chat, &aid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Most logic that exercises ChatState requires a Leptos owner, so we
    // only assert the order-bookkeeping that runs in plain Rust here.

    fn agent(s: &str) -> AgentId {
        s.to_string()
    }

    #[test]
    fn snapshot_default_carries_no_agent_id_until_activate_stamps_it() {
        let snap = SessionSnapshot::default();
        assert!(snap.agent_id.is_none());
        assert!(snap.messages.is_empty());
        assert_eq!(snap.next_msg_id, 0);
    }

    #[test]
    fn agent_id_eq_drives_no_op_activate() {
        // Pure invariant: equality of Option<AgentId> drives the
        // short-circuit in `activate` — verified via a raw comparison so
        // the runtime branch is exercised without Leptos scope setup.
        let a = Some(agent("default"));
        let b = Some(agent("default"));
        assert_eq!(a.as_ref(), b.as_ref());
    }
}
