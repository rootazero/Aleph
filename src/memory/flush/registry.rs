//! Per-agent flush-state registry. A session-end flush registers itself here;
//! a follow-on session's `await_ready` blocks (bounded) until it finishes, so a
//! fast back-to-back session sees consolidated memory while a normal session
//! never waits.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;

use crate::sync_primitives::Arc;

#[derive(Clone, Default)]
pub struct FlushRegistry {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

/// Per-agent flush state. `in_flight` counts the flushes currently running for
/// the agent — sessions are the parallel unit while agents are the storage unit
/// (§4.9), so one agent can have several flushes in flight at once (two of its
/// sessions closing back-to-back, or a `/new` that closes the old session and
/// opens a new one). The agent is ready only once the LAST of them finishes.
struct Entry {
    notify: Arc<Notify>,
    in_flight: usize,
}

/// Held for the flush duration; on drop, wakes all waiters for the agent — but
/// only once every concurrent flush for that agent has also finished.
pub struct FlushGuard {
    notify: Arc<Notify>,
    reg: FlushRegistry,
    agent: String,
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        let mut map = self.reg.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Only the LAST in-flight flush for the agent clears the entry. Removing
        // it unconditionally (the previous shape) evicted a *sibling* flush's
        // live entry: with two overlapping flushes for one agent, the first to
        // finish deleted the second's entry, so `await_ready` saw an empty map
        // and reported ready while the second was still consolidating — the
        // readiness gate silently no-op'd in exactly the multi-session case it
        // exists for, and the eviction cascaded onto any later flush's entry.
        let drained = match map.get_mut(&self.agent) {
            Some(entry) => {
                entry.in_flight = entry.in_flight.saturating_sub(1);
                entry.in_flight == 0
            }
            None => true,
        };

        if drained {
            map.remove(&self.agent);
            drop(map); // release the lock before waking waiters
            self.notify.notify_waiters();
        }
    }
}

impl FlushRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a flush in progress for `agent`. Hold the guard for the flush
    /// duration; drop it when done (the last guard's drop wakes waiters).
    ///
    /// Concurrent flushes for the same agent share one `Notify` and bump
    /// `in_flight`, so a waiter parked on it is woken exactly once, by the last
    /// flush to finish.
    pub fn begin(&self, agent: &str) -> FlushGuard {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(agent.to_string()).or_insert_with(|| Entry {
            notify: Arc::new(Notify::new()),
            in_flight: 0,
        });
        entry.in_flight += 1;
        let notify = entry.notify.clone();
        drop(map);

        FlushGuard {
            notify,
            reg: self.clone(),
            agent: agent.to_string(),
        }
    }

    /// Wait until `agent` has no in-progress flush, or `timeout` elapses.
    /// Returns `true` if ready within the window, `false` on timeout.
    pub async fn await_ready(&self, agent: &str, timeout: Duration) -> bool {
        // Clone the Arc<Notify> while holding the lock. This ensures the
        // notified() future is registered against the same Notify instance
        // that FlushGuard::drop will call notify_waiters() on. tokio::Notify
        // permits creating a Notified future before the notification fires —
        // as long as notified() is polled before notify_waiters() is called,
        // there is no missed-wakeup. The 50ms sleep in the test gives ample
        // time for the spawned task to poll the future before the guard drops.
        let notify = {
            let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match map.get(agent) {
                Some(entry) => entry.notify.clone(),
                None => return true, // idle → ready
            }
        };
        tokio::time::timeout(timeout, notify.notified())
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn await_ready_returns_immediately_when_idle() {
        let reg = FlushRegistry::new();
        let waited = reg.await_ready("main", Duration::from_millis(200)).await;
        assert!(waited, "idle agent is immediately ready");
    }

    #[tokio::test]
    async fn await_ready_blocks_until_flush_done() {
        let reg = FlushRegistry::new();
        let guard = reg.begin("main");
        let reg2 = reg.clone();
        let h = tokio::spawn(async move { reg2.await_ready("main", Duration::from_secs(2)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(guard); // flush completed
        assert!(h.await.unwrap(), "waiter unblocks once flush finishes");
    }

    #[tokio::test]
    async fn await_ready_times_out_if_flush_hangs() {
        let reg = FlushRegistry::new();
        let _guard = reg.begin("main"); // never finishes
        let waited = reg.await_ready("main", Duration::from_millis(100)).await;
        assert!(!waited, "bounded wait gives up");
    }

    /// Two sessions of the SAME agent can end back-to-back (§4.9: sessions are
    /// the parallel unit, agents are the storage unit — same-agent sessions run
    /// truly concurrently). Each `close_session` calls `begin(agent_id)`, so two
    /// flushes for one agent overlap. The first flush finishing must NOT declare
    /// the agent ready while the second is still consolidating.
    #[tokio::test]
    async fn await_ready_blocks_until_the_last_concurrent_flush_of_an_agent_ends() {
        let reg = FlushRegistry::new();
        let g1 = reg.begin("main"); // session A ends
        let g2 = reg.begin("main"); // session B ends, same agent

        drop(g1); // flush A completes; flush B is still in flight

        assert!(
            !reg.await_ready("main", Duration::from_millis(100)).await,
            "agent is NOT ready: flush B is still consolidating"
        );

        drop(g2); // last flush completes
        assert!(
            reg.await_ready("main", Duration::from_millis(100)).await,
            "agent is ready once every in-flight flush has finished"
        );
    }

    /// A waiter that registered before any flush completed must be woken by the
    /// LAST guard drop, not the first.
    #[tokio::test]
    async fn waiter_is_woken_only_after_the_final_concurrent_flush() {
        let reg = FlushRegistry::new();
        let g1 = reg.begin("main");
        let g2 = reg.begin("main");

        let reg2 = reg.clone();
        let h = tokio::spawn(async move { reg2.await_ready("main", Duration::from_secs(2)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        drop(g1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !h.is_finished(),
            "waiter must still be parked after flush A"
        );

        drop(g2);
        assert!(
            h.await.unwrap(),
            "waiter unblocks once flush B also finishes"
        );
    }
}
