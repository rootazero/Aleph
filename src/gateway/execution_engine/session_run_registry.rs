//! Per-session run mutual-exclusion registry.
//!
//! Replaces the per-agent `AgentState` gate (`agent_instance.rs::try_start_run`).
//! Exactly one run may be Running per `SessionKey` at a time (INV-SEQ / audit
//! 4.2: prevents two runs interleaving one session's `session_events`, which
//! would corrupt the transcript). Sessions of the *same* agent no longer
//! contend — they run in parallel (bounded by `ConcurrencyLimiter`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// Tracks the single in-flight run per session. `session_key_string -> run_id`.
#[derive(Default)]
pub(super) struct SessionRunRegistry {
    running: Mutex<HashMap<String, String>>,
    /// Monotonic version stamped under the `running` lock on every effective
    /// claim/release, so a `(seq, keys)` snapshot is internally consistent and
    /// consumers can drop reordered/stale broadcasts.
    seq: AtomicU64,
    /// Optional broadcast sink (injected post-construction, mirrors the
    /// engine's own `event_bus: Option`). When present, every state change
    /// publishes `RunningSetChanged` so the Panel red-dot stays authoritative.
    event_bus: OnceLock<Arc<GatewayEventBus>>,
}

impl SessionRunRegistry {
    /// Inject the broadcast sink once (idempotent no-op if already set). Called
    /// by the engine when its own `event_bus` is wired.
    pub(super) fn set_event_bus(&self, bus: Arc<GatewayEventBus>) {
        let _ = self.event_bus.set(bus);
    }

    /// Internally-consistent `(seq, running_keys)` read under the map lock.
    #[must_use]
    pub(super) fn running_snapshot(&self) -> (u64, Vec<String>) {
        let map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.seq.load(Ordering::Acquire);
        (seq, map.keys().cloned().collect())
    }

    /// Broadcast the current running snapshot to the injected event bus. The
    /// caller (`try_claim`/`release`) has already bumped `seq` and dropped the
    /// map lock; this only reads a fresh `(seq, keys)` snapshot and publishes it,
    /// so the map lock is never held across serialization/broadcast.
    fn broadcast_change(&self) {
        if let Some(bus) = self.event_bus.get() {
            let (seq, running) = self.running_snapshot();
            let _ = bus.publish_frame(&GatewayEventFrame::RunningSetChanged { seq, running });
        }
    }

    /// Atomically claim this session's single run slot. `true` = claimed,
    /// `false` = a run is already active on this session (caller routes the
    /// message to the per-session `BusyInputMode` steer/interrupt/queue path).
    #[must_use]
    pub(super) fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool {
        let key = session_key.to_key_string();
        {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.contains_key(&key) {
                return false;
            }
            map.insert(key.clone(), run_id.to_string());
            self.seq.fetch_add(1, Ordering::AcqRel);
        }
        // Claim is the ONE place a session's run slot is actually taken, so it
        // is the authoritative "this queued message has become a run" edge —
        // the exact mirror of `release`'s `notify_slot_free` below. The busy
        // wait lane holds *waiting* messages; a message that just started
        // running must stop blocking the followers who need to reach the engine
        // while it runs (that is how `Steer` and `Interrupt` work at all). A run
        // that never came through the lane matches no ticket and this is a
        // cheap no-op.
        crate::gateway::busy_queue::mark_admitted(&key, run_id);
        self.broadcast_change();
        true
    }

    /// Release this session's run slot. Idempotent, and only releases when the
    /// stored `run_id` matches — a superseded run's late release can't free a
    /// newer run's claim.
    pub(super) fn release(&self, session_key: &SessionKey, run_id: &str) {
        let key = session_key.to_key_string();
        let changed = {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.get(&key).map(String::as_str) == Some(run_id) {
                map.remove(&key);
                self.seq.fetch_add(1, Ordering::AcqRel);
                true
            } else {
                false
            }
        };
        if changed {
            // Release is the ONE place a session's claim is actually given up,
            // so it is the authoritative "a queued message may try now" edge.
            // Signalling here is what lets the busy wait lane park on a wake
            // instead of re-attempting on a 2-second timer (codex
            // `InputQueueActivity` parity). Cheap: a session with no waiters
            // does nothing.
            crate::gateway::busy_queue::notify_slot_free(&key);
            self.broadcast_change();
        }
    }

    /// Snapshot of every session with a run currently claimed — the live,
    /// authoritative "which sessions are running" set (the in-memory gate, not
    /// the cosmetic persisted store marker, which can go stale on crash).
    ///
    /// Surfaced beside the `ConcurrencyLimiter` snapshot via
    /// `gateway.metrics.run_concurrency` so a Panel can paint per-session
    /// running indicators on a fresh load, and for runs started by another
    /// interface (daemon / Telegram / another Panel) — cases client-side
    /// run-event refcounting alone can't see.
    #[must_use]
    pub(super) fn running_keys(&self) -> Vec<String> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
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
        let key = s.to_key_string();
        assert!(reg.try_claim(&s, "run-A"));
        reg.release(&s, "run-STALE"); // late release from a stale run must not free the current claim
        assert!(
            reg.running_keys().contains(&key),
            "mismatched release must not take effect"
        );
        reg.release(&s, "run-A");
        assert!(!reg.running_keys().contains(&key));
    }

    #[test]
    fn running_keys_lists_every_claimed_session() {
        let reg = SessionRunRegistry::default();
        let a = sk("main", "conv-1");
        let b = sk("other", "conv-2");
        assert!(reg.running_keys().is_empty());
        assert!(reg.try_claim(&a, "run-a"));
        assert!(reg.try_claim(&b, "run-b"));
        let mut keys = reg.running_keys();
        keys.sort();
        let mut want = vec![a.to_key_string(), b.to_key_string()];
        want.sort();
        assert_eq!(keys, want);
    }

    /// The wire that makes mid-loop steering reachable at all: claiming the
    /// session's slot for a queued run withdraws that run's ticket from the
    /// busy wait lane, so the next message becomes the lane's front and can
    /// attempt delivery *while* the first run is still in flight (where the
    /// engine steers or interrupts it). Without this the follower parked behind
    /// a ticket held for the whole run and never reached `admit_run`.
    #[test]
    fn claim_withdraws_the_queued_run_from_its_wait_lane() {
        use crate::gateway::busy_queue;

        let reg = SessionRunRegistry::default();
        let s = sk("lane-agent", "conv-claim");
        let key = s.to_key_string();

        let running = busy_queue::register(&key, 8, "run-1").expect("lane accepts the message");
        let follower = busy_queue::register(&key, 8, "run-2").expect("lane accepts the follow-up");
        assert!(!follower.is_front(), "the follower starts behind");

        assert!(reg.try_claim(&s, "run-1"));
        assert!(
            follower.is_front(),
            "admitting run-1 must let the follow-up reach the engine mid-run"
        );
        assert_eq!(
            busy_queue::purge(&key),
            1,
            "only the still-waiting message is purgeable; the admitted run is not queued"
        );

        drop(running);
        drop(follower);
    }

    #[test]
    fn seq_is_monotonic_across_claim_and_release() {
        let reg = SessionRunRegistry::default();
        let s = sk("main", "conv-1");
        let (seq0, keys0) = reg.running_snapshot();
        assert!(keys0.is_empty());
        assert!(reg.try_claim(&s, "run-1"));
        let (seq1, keys1) = reg.running_snapshot();
        assert!(seq1 > seq0, "claim bumps seq");
        assert_eq!(keys1, vec![s.to_key_string()]);
        reg.release(&s, "run-1");
        let (seq2, keys2) = reg.running_snapshot();
        assert!(seq2 > seq1, "release bumps seq");
        assert!(keys2.is_empty());
        // A no-op release (mismatched run id) must NOT bump seq.
        assert!(reg.try_claim(&s, "run-2"));
        let (seq3, _) = reg.running_snapshot();
        reg.release(&s, "STALE");
        let (seq4, _) = reg.running_snapshot();
        assert_eq!(seq3, seq4, "no-op release does not bump seq");
    }
}
