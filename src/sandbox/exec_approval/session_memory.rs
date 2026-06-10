//! Session-scoped approval memory: remembers a user's "approve for the rest of
//! this session" (`AllowAlways`) choice so a confirm-gated tool is not
//! re-prompted on every call within the same conversation.
//!
//! Maps codex's `ApprovalStore` (`core/src/tools/sandboxing.rs`) onto Aleph: a
//! grant is keyed by the conversation's `SessionKey` string, so it is scoped to
//! one session and never leaks across sessions. The store is bounded so a
//! long-running daemon's footprint stays flat — `AllowAlways` is a deliberate,
//! rare user action, so evicting an old session beyond the cap costs at most a
//! re-prompt, never a wrong grant.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use crate::sync_primitives::Mutex;

/// Max distinct sessions retained before FIFO eviction kicks in.
const MAX_SESSIONS: usize = 1024;

#[derive(Default)]
struct Inner {
    by_session: HashMap<String, HashSet<String>>,
    /// FIFO of session keys, for bounded eviction of the oldest session.
    order: VecDeque<String>,
}

/// Process-wide record of "approve for session" tool grants, keyed by session.
pub struct SessionApprovalMemory {
    inner: Mutex<Inner>,
}

impl SessionApprovalMemory {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// True when `tool` was granted "approve for session" under `session`.
    pub fn is_approved(&self, session: &str, tool: &str) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .by_session
            .get(session)
            .is_some_and(|tools| tools.contains(tool))
    }

    /// Remember that the user approved `tool` for the remainder of `session`.
    pub fn remember(&self, session: &str, tool: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.by_session.contains_key(session) {
            // New session: enforce the bound before inserting so the map and
            // the FIFO order stay in lockstep.
            while guard.order.len() >= MAX_SESSIONS {
                match guard.order.pop_front() {
                    Some(evicted) => {
                        guard.by_session.remove(&evicted);
                    }
                    None => break,
                }
            }
            guard.order.push_back(session.to_string());
        }
        guard
            .by_session
            .entry(session.to_string())
            .or_default()
            .insert(tool.to_string());
    }
}

static GLOBAL: LazyLock<SessionApprovalMemory> = LazyLock::new(SessionApprovalMemory::new);

/// Process-wide session approval memory shared by every per-turn tool service.
pub fn global() -> &'static SessionApprovalMemory {
    &GLOBAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_recalls_per_session() {
        let mem = SessionApprovalMemory::new();
        assert!(!mem.is_approved("s1", "vault_store"));
        mem.remember("s1", "vault_store");
        assert!(mem.is_approved("s1", "vault_store"));
        // Different tool under the same session is not implicitly granted.
        assert!(!mem.is_approved("s1", "agent_delete"));
    }

    #[test]
    fn grants_do_not_leak_across_sessions() {
        let mem = SessionApprovalMemory::new();
        mem.remember("s1", "vault_store");
        // A grant in s1 must never satisfy s2 — the core isolation invariant.
        assert!(!mem.is_approved("s2", "vault_store"));
    }

    #[test]
    fn repeated_remember_is_idempotent() {
        let mem = SessionApprovalMemory::new();
        mem.remember("s1", "vault_store");
        mem.remember("s1", "vault_store");
        assert!(mem.is_approved("s1", "vault_store"));
        let guard = mem.inner.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(guard.order.len(), 1, "same session enqueued once");
        assert_eq!(guard.by_session.get("s1").map(|t| t.len()), Some(1));
    }

    #[test]
    fn bounded_eviction_drops_oldest_session() {
        let mem = SessionApprovalMemory::new();
        // Fill to the cap, then add one more to trigger one FIFO eviction.
        for i in 0..MAX_SESSIONS {
            mem.remember(&format!("s{i}"), "tool");
        }
        assert!(mem.is_approved("s0", "tool"));
        mem.remember("overflow", "tool");
        // Oldest (s0) evicted; newest retained.
        assert!(!mem.is_approved("s0", "tool"));
        assert!(mem.is_approved("overflow", "tool"));
        let guard = mem.inner.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(guard.order.len(), MAX_SESSIONS);
        assert_eq!(guard.by_session.len(), MAX_SESSIONS);
    }
}
