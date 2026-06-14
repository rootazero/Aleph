//! Loop subsystem: a per-session, timer-driven repeat managed by the LLM via
//! the `loop` tool (R8) and re-fired each turn by the execution engine's
//! continuation hook. The clock-gated sibling of `goal` (condition-gated).
//!
//! Storage is PROCESS MEMORY ONLY — never `tasks.db`. A daemon restart clears
//! every loop, which is the physical guarantee of the "随会话消亡" semantics.

pub mod pursuit;
pub mod types;

pub use types::{Cadence, LoopState, LoopStatus};

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory map of `session_key` → `LoopState`.
#[derive(Default)]
pub struct LoopRegistry {
    inner: Mutex<HashMap<String, LoopState>>,
}

impl LoopRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, LoopState>> {
        // Poison-safe (project rule P7): recover the guard rather than panic.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert or overwrite the loop for a session.
    pub fn put(&self, state: LoopState) {
        self.lock().insert(state.session_id.clone(), state);
    }

    /// Read the loop for a session regardless of status.
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<LoopState> {
        self.lock().get(session_id).cloned()
    }

    /// Read the loop only if it is `Active`.
    #[must_use]
    pub fn get_active(&self, session_id: &str) -> Option<LoopState> {
        self.lock()
            .get(session_id)
            .filter(|l| l.is_active())
            .cloned()
    }

    /// Drop the loop for a session.
    pub fn remove(&self, session_id: &str) {
        self.lock().remove(session_id);
    }
}

/// Process-global registry. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// loop subsystem" and the continuation hook stays dormant.
static GLOBAL: OnceCell<Arc<LoopRegistry>> = OnceCell::new();

/// Install the global registry at boot. Idempotent.
pub fn init_global(registry: Arc<LoopRegistry>) {
    let _ = GLOBAL.set(registry);
}

/// Read the global registry, if initialized.
#[must_use]
pub fn global() -> Option<Arc<LoopRegistry>> {
    GLOBAL.get().cloned()
}

/// Test-only override.
#[cfg(test)]
pub fn set_global_for_test(registry: Arc<LoopRegistry>) {
    let _ = GLOBAL.set(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    fn st(sess: &str) -> LoopState {
        LoopState::new(sess, "p", Cadence::Fixed { interval_ms: 1000 }, 0)
    }

    #[test]
    fn put_then_get_returns_state() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        assert_eq!(reg.get("a").unwrap().session_id, "a");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn put_overwrites_same_session() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.put(st("a").spent_iteration());
        assert_eq!(reg.get("a").unwrap().iterations_used, 1);
    }

    #[test]
    fn remove_deletes_state() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.remove("a");
        assert!(reg.get("a").is_none());
    }

    #[test]
    fn get_active_filters_stopped() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_status(LoopStatus::Stopped));
        assert!(reg.get("a").is_some(), "get returns regardless of status");
        assert!(reg.get_active("a").is_none(), "get_active skips Stopped");
    }
}
