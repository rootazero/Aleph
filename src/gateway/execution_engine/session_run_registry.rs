//! Per-session run mutual-exclusion registry.
//!
//! Replaces the per-agent `AgentState` gate (`agent_instance.rs::try_start_run`).
//! Exactly one run may be Running per `SessionKey` at a time (INV-SEQ / audit
//! 4.2: prevents two runs interleaving one session's `session_events`, which
//! would corrupt the transcript). Sessions of the *same* agent no longer
//! contend — they run in parallel (bounded by `ConcurrencyLimiter`).

use std::collections::HashMap;

use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// Tracks the single in-flight run per session. `session_key_string -> run_id`.
#[derive(Default)]
pub(super) struct SessionRunRegistry {
    running: Mutex<HashMap<String, String>>,
}

impl SessionRunRegistry {
    /// Atomically claim this session's single run slot. `true` = claimed,
    /// `false` = a run is already active on this session (caller routes the
    /// message to the per-session `BusyInputMode` steer/interrupt/queue path).
    #[must_use]
    pub(super) fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool {
        let key = session_key.to_key_string();
        let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(&key) {
            return false;
        }
        map.insert(key, run_id.to_string());
        true
    }

    /// Release this session's run slot. Idempotent, and only releases when the
    /// stored `run_id` matches — a superseded run's late release can't free a
    /// newer run's claim.
    pub(super) fn release(&self, session_key: &SessionKey, run_id: &str) {
        let key = session_key.to_key_string();
        let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(&key).map(String::as_str) == Some(run_id) {
            map.remove(&key);
        }
    }

    /// Is a run currently active on this session?
    ///
    /// Not yet called from production code (`try_claim`'s return value already
    /// covers the gate's own need); kept for diagnostics / a future status RPC.
    /// `#[allow(dead_code)]` stays until it gets a non-test caller (mirrors
    /// `concurrency::ConcurrencyLimiter::snapshot`).
    #[must_use]
    #[allow(dead_code)]
    pub(super) fn is_running(&self, session_key: &SessionKey) -> bool {
        let key = session_key.to_key_string();
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `(agent, conversation)` session key. `SessionKey::peer` is the
    /// real constructor for a per-peer DM session, keyed by `peer_id` — two
    /// different `conv` values under the SAME `agent` produce two distinct
    /// `SessionKey`s (distinct `to_key_string()`) while `agent_id()` stays
    /// identical, which is exactly the "same agent, different session" shape
    /// this registry must not contend on.
    fn sk(agent: &str, conv: &str) -> SessionKey {
        SessionKey::peer(agent, conv)
    }

    #[test]
    fn claim_is_exclusive_per_session_but_free_across_sessions() {
        let reg = SessionRunRegistry::default();
        let a1 = sk("main", "conv-1");
        let a2 = sk("main", "conv-2"); // same agent, different session

        // Guard against a fake test: a1/a2 must be genuinely distinct sessions
        // of the genuinely same agent, not accidentally the same key twice.
        assert_eq!(a1.agent_id(), a2.agent_id(), "must be the same agent");
        assert_ne!(a1, a2, "must be two distinct sessions");

        assert!(reg.try_claim(&a1, "run-1"));
        assert!(
            !reg.try_claim(&a1, "run-1b"),
            "second claim on the same session must be rejected"
        );
        assert!(
            reg.try_claim(&a2, "run-2"),
            "same agent, different session must be admitted (true parallelism)"
        );

        reg.release(&a1, "run-1");
        assert!(
            reg.try_claim(&a1, "run-3"),
            "claim must succeed again after release"
        );
    }

    #[test]
    fn release_only_matching_run_id() {
        let reg = SessionRunRegistry::default();
        let s = sk("main", "conv-1");
        assert!(reg.try_claim(&s, "run-A"));
        reg.release(&s, "run-STALE"); // late release from a stale run must not free the current claim
        assert!(reg.is_running(&s), "mismatched release must not take effect");
        reg.release(&s, "run-A");
        assert!(!reg.is_running(&s));
    }
}
