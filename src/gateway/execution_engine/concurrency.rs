//! Run-lifetime concurrency limiter.
//!
//! Unlike the RPC lane permit (`lane.rs`, released at dispatch — audit 1.4),
//! a `RunPermit` is held for the whole run (acquired at the gate, dropped when
//! `execute()` returns). Two caps stack: a `global` semaphore and a per-agent
//! sub-cap so one busy agent can't monopolize all global slots (audit C4).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::sync_primitives::Mutex;

/// Snapshot of the limiter's slot usage, surfaced via
/// `gateway.metrics.run_concurrency` (Task 8, audit 3.4) — "N/M run slots in
/// use" for ops dashboards / Panel UIs. `pub` (not module-local): reached
/// through the public `ExecutionAdapter` trait and `ExecutionEngine` (both
/// externally-reachable types), so the return type must be at least as
/// visible — mirrors `LaneOccupancy` (`lane.rs`), the sibling diagnostics
/// snapshot for `gateway.metrics.lanes`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ConcurrencySnapshot {
    pub global_in_use: usize,
    pub global_total: usize,
    /// The per-agent sub-cap (audit C4): the max concurrent runs one agent may
    /// hold across its sessions. The agent id is the memory/storage
    /// physical-isolation boundary (a session runs under exactly one agent), so
    /// this cap bounds contention on that boundary — distinct from the
    /// per-session parallelism the `SessionRunRegistry` governs.
    pub per_agent_cap: usize,
    /// Runs currently blocked in `acquire().await` because both caps were full
    /// when they were admitted — the queue depth behind the semaphores. `0` in
    /// the common unsaturated case; a rising value is the early-warning signal
    /// that surfaced the previously-untyped `RunQueued` TODO (audit 1.2 keeps
    /// the run queued rather than rejected).
    pub waiting: usize,
    /// Per-agent in-use slot counts, only for agents holding ≥1 run, sorted by
    /// usage desc then agent id. Lets the dashboard see *which* isolation
    /// boundary is saturating, not just the global total.
    pub per_agent: Vec<AgentSlotUsage>,
}

/// One agent's live run-slot usage within the limiter (a row of
/// [`ConcurrencySnapshot::per_agent`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSlotUsage {
    pub agent_id: String,
    pub in_use: usize,
}

pub(super) struct RunPermit {
    _global: OwnedSemaphorePermit,
    _agent: OwnedSemaphorePermit,
}

/// Increments `waiting` on construction and decrements on drop, so a run
/// blocked in [`ConcurrencyLimiter::acquire`] is counted for exactly its wait
/// duration — and a caller future dropped mid-await can't leak a phantom
/// waiter (Drop runs on unwind too).
struct WaitGuard<'a>(&'a AtomicUsize);

impl<'a> WaitGuard<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct ConcurrencyLimiter {
    /// Hot-swappable so `reconfigure` can resize (tokio 1.35 `Semaphore` grows
    /// via `add_permits` but cannot shrink; a whole-semaphore swap is the
    /// version-safe resize). In-flight permits held against the previous
    /// `Arc<Semaphore>` keep it alive until they drop, so a shrink overshoots
    /// transiently by at most the old in-flight count, then converges.
    global: ArcSwap<Semaphore>,
    global_total: AtomicUsize,
    per_agent_cap: AtomicUsize,
    per_agent: Mutex<HashMap<String, Arc<Semaphore>>>,
    /// Live count of runs blocked in `acquire().await` (queue depth).
    waiting: AtomicUsize,
}

impl ConcurrencyLimiter {
    #[must_use]
    pub(super) fn new(global_cap: usize, per_agent_cap: usize) -> Self {
        // Clamp to >=1 (a 0-permit semaphore would deadlock every run).
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        Self {
            global: ArcSwap::from_pointee(Semaphore::new(global_cap)),
            global_total: AtomicUsize::new(global_cap),
            per_agent_cap: AtomicUsize::new(per_agent_cap),
            per_agent: Mutex::new(HashMap::new()),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Live-resize both caps (hot-reload of `[execution] max_runs_*`). The
    /// global semaphore is swapped wholesale; the per-agent map is cleared so
    /// each agent's sub-semaphore rebuilds lazily at the new cap. In-flight
    /// permits against the old semaphores stay valid until dropped.
    pub(super) fn reconfigure(&self, global_cap: usize, per_agent_cap: usize) {
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        self.global.store(Arc::new(Semaphore::new(global_cap)));
        self.global_total.store(global_cap, Ordering::Release);
        self.per_agent_cap.store(per_agent_cap, Ordering::Release);
        self.per_agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn agent_sem(&self, agent_id: &str) -> Arc<Semaphore> {
        let mut map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
        // The cap MUST be read while holding the map lock: `reconfigure`
        // stores the new cap BEFORE it locks-and-clears the map, so an
        // admission that loaded the cap outside the lock could interleave as
        // load(OLD) → reconfigure stores NEW + clears → or_insert(Sem(OLD)) —
        // installing a stale-cap semaphore into the fresh map that persists
        // until the NEXT reconfigure (and skews `snapshot().per_agent`, which
        // computes in_use against the NEW cap). Under the lock, either this
        // insert happens before the clear (and is wiped), or after it (and
        // reads the new cap) — the stale insert is impossible.
        let cap = self.per_agent_cap.load(Ordering::Acquire);
        map.entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(cap)))
            .clone()
    }

    /// Acquire both a global and a per-agent permit, awaiting if either cap is
    /// full. The per-agent permit is taken first so a saturated agent waits on
    /// its own sub-cap without consuming a scarce global slot. Time spent
    /// blocked here is reflected in `snapshot().waiting`.
    ///
    /// # Order trade-off (reviewed; intentional)
    ///
    /// Agent-first means a run parked on the GLOBAL semaphore holds its agent
    /// permit while it waits. The audit flagged the corner: with
    /// `max_runs_per_agent=3, max_runs_global=1`, three runs of agent A each
    /// hold an agent slot and queue on the single global slot, so a fourth
    /// agent-A run is rejected (agent cap reads full) even though only one is
    /// executing. That is real, and it is the price of the invariant this
    /// order protects: a saturated agent must NEVER hold a global slot while
    /// merely *waiting* for admission. The reverse order (global-first) would
    /// let a flood of one agent's runs occupy every global slot in the wait
    /// queue and starve every OTHER agent out of the global semaphore — the
    /// cross-tenant failure mode this limiter exists to prevent. Agent-first
    /// contains the blast radius to the offending agent (its own extra runs
    /// fail fast); global-first spreads it to all tenants. Given the defaults
    /// (`global=8, per_agent=3`) the flagged corner needs an aggressively
    /// mis-sized config to matter, and the fix direction would reopen the
    /// worse failure. Keeping agent-first; this note is the record of the
    /// decision.
    pub(super) async fn acquire(&self, agent_id: &str) -> RunPermit {
        let _wait = WaitGuard::enter(&self.waiting);
        let agent_sem = self.agent_sem(agent_id);
        let agent = agent_sem
            .acquire_owned()
            .await
            .expect("agent sem never closed");
        // Clone the current global semaphore Arc, then await on it — never hold
        // the ArcSwap guard across the await (a concurrent reconfigure must not
        // be blocked, and the permit binds to whichever semaphore was live).
        let global_sem = self.global.load_full();
        let global = global_sem
            .acquire_owned()
            .await
            .expect("global sem never closed");
        RunPermit {
            _global: global,
            _agent: agent,
        }
    }

    /// Non-blocking variant. Returns `None` if either cap is currently full.
    #[must_use]
    pub(super) fn try_acquire(&self, agent_id: &str) -> Option<RunPermit> {
        let agent_sem = self.agent_sem(agent_id);
        let agent = Arc::clone(&agent_sem).try_acquire_owned().ok()?;
        let global_sem = self.global.load_full();
        let global = global_sem.try_acquire_owned().ok()?;
        Some(RunPermit {
            _global: global,
            _agent: agent,
        })
    }

    #[must_use]
    pub(super) fn snapshot(&self) -> ConcurrencySnapshot {
        let per_agent_cap = self.per_agent_cap.load(Ordering::Acquire);
        let global_sem = self.global.load_full();
        let global_total = self.global_total.load(Ordering::Acquire);
        let per_agent = {
            let map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
            let mut rows: Vec<AgentSlotUsage> = map
                .iter()
                .filter_map(|(id, sem)| {
                    let in_use = per_agent_cap.saturating_sub(sem.available_permits());
                    (in_use > 0).then(|| AgentSlotUsage {
                        agent_id: id.clone(),
                        in_use,
                    })
                })
                .collect();
            rows.sort_by(|a, b| {
                b.in_use
                    .cmp(&a.in_use)
                    .then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            rows
        };
        ConcurrencySnapshot {
            global_in_use: global_total.saturating_sub(global_sem.available_permits()),
            global_total,
            per_agent_cap,
            waiting: self.waiting.load(Ordering::Acquire),
            per_agent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn global_cap_bounds_total_and_releases_on_drop() {
        let lim = ConcurrencyLimiter::new(2, 5);
        let p1 = lim.try_acquire("main").expect("slot 1");
        let p2 = lim.try_acquire("other").expect("slot 2");
        assert!(lim.try_acquire("third").is_none(), "global cap=2 已满");
        assert_eq!(lim.snapshot().global_in_use, 2);
        drop(p1);
        assert!(lim.try_acquire("third").is_some(), "drop 释放全局槽");
        drop(p2);
    }

    #[tokio::test]
    async fn per_agent_cap_bounds_one_agent_without_starving_others() {
        let lim = ConcurrencyLimiter::new(10, 2);
        let _a1 = lim.try_acquire("main").unwrap();
        let _a2 = lim.try_acquire("main").unwrap();
        assert!(lim.try_acquire("main").is_none(), "per-agent cap=2 已满");
        assert!(
            lim.try_acquire("other").is_some(),
            "别的 agent 不受 main 影响"
        );
    }

    #[tokio::test]
    async fn snapshot_reports_per_agent_usage_sorted_and_idle_omitted() {
        let lim = ConcurrencyLimiter::new(10, 3);
        let _m1 = lim.try_acquire("main").unwrap();
        let _m2 = lim.try_acquire("main").unwrap();
        let _o1 = lim.try_acquire("other").unwrap();
        // Touch a third agent then release, so its semaphore is resident but idle.
        drop(lim.try_acquire("idle").unwrap());

        let snap = lim.snapshot();
        assert_eq!(snap.per_agent_cap, 3);
        assert_eq!(snap.global_in_use, 3);
        // Idle agent omitted; busiest first, ties broken by id.
        assert_eq!(snap.per_agent.len(), 2, "只列有活跃 run 的 agent");
        assert_eq!(snap.per_agent[0].agent_id, "main");
        assert_eq!(snap.per_agent[0].in_use, 2);
        assert_eq!(snap.per_agent[1].agent_id, "other");
        assert_eq!(snap.per_agent[1].in_use, 1);
    }

    #[tokio::test]
    async fn reconfigure_grows_and_shrinks_caps() {
        let lim = ConcurrencyLimiter::new(1, 1);
        let _p1 = lim.try_acquire("main").expect("slot 1");
        assert!(lim.try_acquire("other").is_none(), "global cap=1 已满");

        // Grow global to 3 → a new agent can now acquire.
        lim.reconfigure(3, 2);
        assert_eq!(lim.snapshot().global_total, 3);
        assert_eq!(lim.snapshot().per_agent_cap, 2);
        let _p2 = lim.try_acquire("other").expect("grown global slot");
        // Old in-flight permit still valid (held against the pre-swap semaphore).
        drop(_p1);

        // Shrink global to 1: new acquires bounded by the new semaphore.
        lim.reconfigure(1, 1);
        assert_eq!(lim.snapshot().global_total, 1);
    }

    #[tokio::test]
    async fn reconfigure_rebuilds_per_agent_caps() {
        let lim = ConcurrencyLimiter::new(10, 1);
        let _a1 = lim.try_acquire("main").unwrap();
        assert!(lim.try_acquire("main").is_none(), "per-agent cap=1 已满");
        // Raise per-agent cap → the same agent gets a fresh semaphore at cap 3.
        lim.reconfigure(10, 3);
        let _a2 = lim.try_acquire("main").expect("re-capped agent slot");
        let _a3 = lim.try_acquire("main").expect("re-capped agent slot");
    }

    #[tokio::test]
    async fn waiting_counts_blocked_acquires_and_clears_on_acquire() {
        let lim = Arc::new(ConcurrencyLimiter::new(1, 5));
        let held = lim.try_acquire("main").expect("slot 1");
        assert_eq!(lim.snapshot().waiting, 0);

        let lim2 = Arc::clone(&lim);
        let blocked = tokio::spawn(async move { lim2.acquire("other").await });

        // Poll until the spawned task parks on the full global semaphore.
        let mut saw_wait = false;
        for _ in 0..100 {
            if lim.snapshot().waiting == 1 {
                saw_wait = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(saw_wait, "blocked acquire must register as one waiter");

        drop(held);
        let _permit = blocked
            .await
            .expect("blocked acquire completes after release");
        assert_eq!(lim.snapshot().waiting, 0, "waiter cleared once it acquires");
    }
}
