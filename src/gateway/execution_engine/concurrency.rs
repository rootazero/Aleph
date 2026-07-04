//! Run-lifetime concurrency limiter.
//!
//! Unlike the RPC lane permit (`lane.rs`, released at dispatch — audit 1.4),
//! a `RunPermit` is held for the whole run (acquired at the gate, dropped when
//! `execute()` returns). Two caps stack: a `global` semaphore and a per-agent
//! sub-cap so one busy agent can't monopolize all global slots (audit C4).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::sync_primitives::Mutex;

#[allow(dead_code)] // wired in Task 8 (concurrency snapshot in status/metrics)
pub(super) struct ConcurrencySnapshot {
    pub(super) global_in_use: usize,
    pub(super) global_total: usize,
}

#[allow(dead_code)] // wired in Task 6 (gate acquires/holds for run lifetime)
pub(super) struct RunPermit {
    _global: OwnedSemaphorePermit,
    _agent: OwnedSemaphorePermit,
}

#[allow(dead_code)] // wired in Task 6 (core gate rewrite)
pub(super) struct ConcurrencyLimiter {
    global: Arc<Semaphore>,
    global_total: usize,
    per_agent_cap: usize,
    per_agent: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ConcurrencyLimiter {
    #[must_use]
    #[allow(dead_code)] // wired in Task 6
    pub(super) fn new(global_cap: usize, per_agent_cap: usize) -> Self {
        // Clamp to >=1 (a 0-permit semaphore would deadlock every run).
        let global_cap = global_cap.max(1);
        let per_agent_cap = per_agent_cap.max(1);
        Self {
            global: Arc::new(Semaphore::new(global_cap)),
            global_total: global_cap,
            per_agent_cap,
            per_agent: Mutex::new(HashMap::new()),
        }
    }

    #[allow(dead_code)] // only reachable via acquire/try_acquire, wired in Task 6
    fn agent_sem(&self, agent_id: &str) -> Arc<Semaphore> {
        let mut map = self.per_agent.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_agent_cap)))
            .clone()
    }

    /// Acquire both a global and a per-agent permit, awaiting if either cap is
    /// full. The per-agent permit is taken first so a saturated agent waits on
    /// its own sub-cap without consuming a scarce global slot.
    #[allow(dead_code)] // wired in Task 6
    pub(super) async fn acquire(&self, agent_id: &str) -> RunPermit {
        let agent_sem = self.agent_sem(agent_id);
        let agent = agent_sem.acquire_owned().await.expect("agent sem never closed");
        let global = self.global.clone().acquire_owned().await.expect("global sem never closed");
        RunPermit { _global: global, _agent: agent }
    }

    /// Non-blocking variant. Returns `None` if either cap is currently full.
    #[must_use]
    #[allow(dead_code)] // wired in Task 6
    pub(super) fn try_acquire(&self, agent_id: &str) -> Option<RunPermit> {
        let agent_sem = self.agent_sem(agent_id);
        let agent = Arc::clone(&agent_sem).try_acquire_owned().ok()?;
        let global = self.global.clone().try_acquire_owned().ok()?;
        Some(RunPermit { _global: global, _agent: agent })
    }

    #[must_use]
    #[allow(dead_code)] // wired in Task 8
    pub(super) fn snapshot(&self) -> ConcurrencySnapshot {
        ConcurrencySnapshot {
            global_in_use: self.global_total - self.global.available_permits(),
            global_total: self.global_total,
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
        assert!(lim.try_acquire("other").is_some(), "别的 agent 不受 main 影响");
    }
}
