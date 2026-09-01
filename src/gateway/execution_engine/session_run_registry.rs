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
    ///
    /// Used by tests; production broadcasts go through [`Self::publish_snapshot`]
    /// with the snapshot captured at the seq-bump site.
    #[must_use]
    #[allow(dead_code)]
    pub(super) fn running_snapshot(&self) -> (u64, Vec<String>) {
        let map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.seq.load(Ordering::Acquire);
        (seq, map.keys().cloned().collect())
    }

    /// Publish the supplied (seq, running) snapshot to the injected bus.
    ///
    /// **Audit fix**: the previous `broadcast_change` re-read `running_snapshot`
    /// AFTER the caller had dropped the lock. With two state transitions
    /// interleaving (e.g. claim N → release N+1 before claim's broadcast lands),
    /// the late re-read picked up `(N+1, [])` and the claim's frame was
    /// emitted with the just-claimed run already gone — the Panel saw the
    /// release shadow its own claim. Consumers reconstruct ordering from `seq`
    /// alone, so the snapshot MUST be captured atomically with the seq bump in
    /// `try_claim`/`release` and passed into this helper. The map lock is
    /// never held across `publish_frame` (which serializes and broadcasts);
    /// the seq bump + snapshot capture happens under the lock, then the lock
    /// drops and we publish.
    fn publish_snapshot(&self, seq: u64, running: Vec<String>) {
        if let Some(bus) = self.event_bus.get() {
            let _ = bus.publish_frame(&GatewayEventFrame::RunningSetChanged { seq, running });
        }
    }

    /// Re-publish the CURRENT running set under a FRESH sequence number.
    ///
    /// [`Self::try_claim`]'s broadcast is the first statement of
    /// `ExecutionEngine::admit_run`, several `.await` points before
    /// `AgentInstance::ensure_session` writes the session's row. The
    /// per-connection projection
    /// (`event_visibility::EventVisibilityIndex::project_for`) drops any
    /// element whose row cannot be resolved — the fail-closed direction, and
    /// correct — so on the first turn of a BRAND-NEW conversation that claim
    /// frame reaches every socket with the new key already removed. Nothing
    /// re-fetches: for a single in-flight run the next `RunningSetChanged` is
    /// the RELEASE at run end, which excludes the key too, and the Panel's
    /// cold-load seed only applies before any frame has arrived. So the row
    /// becoming resolvable needs its own frame, published by `execute.rs`
    /// immediately after `ensure_session`.
    ///
    /// The seq bump is load-bearing, not cosmetic: `SessionMap::
    /// set_server_running` discards any frame whose `seq` is `<=` the one it
    /// already holds, so re-publishing at the claim's seq would be a silent
    /// no-op on every connected Panel.
    pub(super) fn republish_running_set(&self) {
        let (seq, snap) = {
            let map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            let new_seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
            let keys = map.keys().cloned().collect::<Vec<_>>();
            (new_seq, keys)
        };
        self.publish_snapshot(seq, snap);
    }

    /// Atomically claim this session's single run slot. `true` = claimed,
    /// `false` = a run is already active on this session (caller routes the
    /// message to the per-session `BusyInputMode` steer/interrupt/queue path).
    ///
    /// **Audit fix**: the snapshot is captured under the same lock as the
    /// seq bump so a racing `release` cannot erase the claim's broadcast
    /// (see [`Self::publish_snapshot`] for the full TOCTOU description).
    #[must_use]
    pub(super) fn try_claim(&self, session_key: &SessionKey, run_id: &str) -> bool {
        let key = session_key.to_key_string();
        let (seq, snap) = {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.contains_key(&key) {
                return false;
            }
            map.insert(key.clone(), run_id.to_string());
            let new_seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
            let keys = map.keys().cloned().collect::<Vec<_>>();
            (new_seq, keys)
        };
        // Claim is the ONE place a session's run slot is actually taken, so it
        // is the authoritative "this queued message has become a run" edge —
        // the exact mirror of `release`'s `notify_slot_free` below. The busy
        // wait lane holds *waiting* messages; a message that just started
        // running must stop blocking the followers who need to reach the engine
        // while it runs (that is how `Steer` and `Interrupt` work at all). A run
        // that never came through the lane matches no ticket and this is a
        // cheap no-op.
        crate::gateway::busy_queue::mark_admitted(&key, run_id);
        self.publish_snapshot(seq, snap);
        true
    }

    /// Release this session's run slot. Idempotent, and only releases when the
    /// stored `run_id` matches — a superseded run's late release can't free a
    /// newer run's claim.
    ///
    /// **Audit fix**: snapshot is captured under the same lock as the seq bump
    /// so a racing `try_claim` cannot erase the release's broadcast.
    pub(super) fn release(&self, session_key: &SessionKey, run_id: &str) {
        let key = session_key.to_key_string();
        let release_seq_snap = {
            let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if map.get(&key).map(String::as_str) != Some(run_id) {
                return;
            }
            map.remove(&key);
            let new_seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
            let keys = map.keys().cloned().collect::<Vec<_>>();
            Some((new_seq, keys))
        };
        if let Some((seq, snap)) = release_seq_snap {
            // Release is the ONE place a session's claim is actually given up,
            // so it is the authoritative "a queued message may try now" edge.
            // Signalling here is what lets the busy wait lane park on a wake
            // instead of re-attempting on a 2-second timer (codex
            // `InputQueueActivity` parity). Cheap: a session with no waiters
            // does nothing.
            crate::gateway::busy_queue::notify_slot_free(&key);
            self.publish_snapshot(seq, snap);
        }
    }

    /// The run id currently claiming `session_key`, if any.
    ///
    /// Point lookup twin of [`Self::running_keys`]: that one answers "which
    /// sessions are busy" (the sidebar dot), this one answers "and which run
    /// is it" for ONE session the caller has already been cleared to address.
    /// Reading the claim map is what makes it authoritative for runs started
    /// by any interface — the Panel's own `AgentRunManager::active_runs` only
    /// ever holds runs the Panel itself started.
    #[must_use]
    pub(super) fn run_id_for(&self, session_key: &str) -> Option<String> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_key)
            .cloned()
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
    ///
    /// This returns the WHOLE process's set. That handler narrows it to the
    /// calling user before it leaves the process, the same way `broadcast_change`'s
    /// frame is narrowed per socket (`event_visibility::EventVisibilityIndex::
    /// project_for`) — the two are one decision, because they seed and then
    /// maintain the same red dot.
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

    /// The point lookup a client joining a session mid-turn depends on. It has
    /// to answer for **any** interface's run, which is why it reads this
    /// registry (the single admission gate) and not `AgentRunManager::
    /// active_runs` (Panel-started runs only) — and it must not answer for a
    /// session that merely happens to have had a run at some point.
    #[test]
    fn run_id_for_names_the_claiming_run_and_nothing_else() {
        let reg = SessionRunRegistry::default();
        let busy = sk("main", "conv-busy");
        let idle = sk("main", "conv-idle");

        assert_eq!(reg.run_id_for(&busy.to_key_string()), None, "nothing yet");
        assert!(reg.try_claim(&busy, "run-live"));
        assert_eq!(
            reg.run_id_for(&busy.to_key_string()),
            Some("run-live".to_string())
        );
        assert_eq!(
            reg.run_id_for(&idle.to_key_string()),
            None,
            "a different session must not inherit the answer"
        );
        assert_eq!(
            reg.run_id_for("agent:main:peer:never-seen"),
            None,
            "and neither must an unknown key"
        );

        reg.release(&busy, "run-live");
        assert_eq!(
            reg.run_id_for(&busy.to_key_string()),
            None,
            "a finished turn is not one a client should try to join"
        );
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

    /// Regression: the running dot for the FIRST turn of a brand-new
    /// conversation.
    ///
    /// `try_claim` is the first statement of `ExecutionEngine::admit_run`,
    /// two `.await` points before `ensure_session` writes the session row, so
    /// the claim's frame is projected per connection
    /// (`event_visibility::EventVisibilityIndex::project_for`) against a key
    /// that resolves to nothing and is dropped — fail-closed and correct, but
    /// it means the socket receives `running: []`. Nothing re-fetched it: the
    /// next `RunningSetChanged` for a single in-flight run is the release at
    /// run end (which excludes the key too) and the Panel's cold-load seed
    /// only applies before any frame has arrived. The dot stayed dark for the
    /// whole turn — on a plain single-user loopback box too, where the caller
    /// resolves to `Some(OWNER_USER_ID)`.
    ///
    /// Both assertions are on the DELIVERED, PROJECTED payload — the bytes a
    /// Panel acts on — plus the seq, because `SessionMap::set_server_running`
    /// discards any frame whose seq is `<=` the one it already recorded: a
    /// re-publish that reused the claim's seq would be an inert fix.
    #[tokio::test]
    async fn a_new_sessions_key_reaches_its_owner_once_its_row_exists() {
        use crate::gateway::event_visibility::EventVisibilityIndex;
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        use crate::gateway::session_store::SessionStore;

        const RUNNING_SET_METHOD: &str = "stream.running_set_changed";

        fn delivered(rx: &mut tokio::sync::broadcast::Receiver<String>) -> serde_json::Value {
            let raw = rx.try_recv().expect("publish_frame delivers synchronously");
            let wire: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(
                wire["method"], RUNNING_SET_METHOD,
                "the registry's only frame is the running set"
            );
            wire
        }

        async fn projected_keys(
            index: &EventVisibilityIndex,
            wire: &serde_json::Value,
            store: &Arc<dyn SessionStore>,
        ) -> Vec<String> {
            let payload = index
                .project_for(
                    wire["method"].as_str().unwrap(),
                    wire.get("params"),
                    Some("u-newcomer"),
                    store,
                )
                .await
                .expect("the running set is always projected, never waved through");
            payload["running"]
                .as_array()
                .expect("a projected frame still carries a `running` array")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        }

        let temp = tempfile::TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();

        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let reg = SessionRunRegistry::default();
        reg.set_event_bus(Arc::clone(&bus));

        let session = sk("newcomer-agent", "conv-first-ever");
        let key = session.to_key_string();
        // ONE index across both frames on purpose: an absent row must not be
        // cached as a permanent denial, or the repair frame would be dropped too.
        let index = EventVisibilityIndex::new();

        // 1. Admission claims the slot while the row still does not exist.
        assert!(reg.try_claim(&session, "run-first"));
        let claim_frame = delivered(&mut rx);
        let store_dyn: Arc<dyn SessionStore> = Arc::new(store);
        assert!(
            projected_keys(&index, &claim_frame, &store_dyn)
                .await
                .is_empty(),
            "the premise: at claim time the key resolves to nobody and is dropped"
        );

        // 2. `execute.rs` creates the row (`agent.ensure_session`).
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-newcomer")),
            store_dyn.get_or_create(&session),
        )
        .await
        .unwrap();

        // 3. …and immediately re-publishes. THIS is the wire under test.
        reg.republish_running_set();
        let repair_frame = delivered(&mut rx);
        assert_eq!(
            projected_keys(&index, &repair_frame, &store_dyn).await,
            vec![key],
            "the owner must be told their brand-new session is running"
        );
        assert!(
            repair_frame["params"]["seq"].as_u64().unwrap()
                > claim_frame["params"]["seq"].as_u64().unwrap(),
            "a re-publish at the claim's seq is discarded by the client and fixes nothing"
        );
    }

    /// The production wire for the frame the test above proves is needed:
    /// `execute.rs` must re-publish AFTER `ensure_session`, not before. A
    /// source-level pin because the full `ExecutionEngine::execute` needs a
    /// live orchestrator to run a turn and so has no unit-level harness —
    /// without this, deleting the call leaves every test green.
    #[test]
    fn execute_republishes_the_running_set_after_creating_the_session_row() {
        let src = include_str!("execute.rs");
        // The needle is the scoped helper, not `agent.ensure_session` directly:
        // creating the row without `run_loop::ensure_session_under_request_scope`
        // leaves `owner_user_id`/`scope_id` NULL (the task-local does not cross
        // the producers' `tokio::spawn`), and an unstamped row is exactly what
        // the projection below cannot resolve. Both properties are about the
        // same statement, so they share one anchor.
        let ensure = src
            .find("run_loop::ensure_session_under_request_scope(&agent, &request).await;")
            .expect("execute.rs still creates the session row under the run's own scope");
        let republish = src
            .find("session_run_registry.republish_running_set()")
            .expect("execute.rs must re-publish the running set once the row exists");
        assert!(
            republish > ensure,
            "the re-publish must run AFTER the row exists, or it projects to nothing again"
        );
    }
}
