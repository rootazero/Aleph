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
    inner: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

/// Held for the flush duration; on drop, wakes all waiters for the agent.
pub struct FlushGuard {
    notify: Arc<Notify>,
    reg: FlushRegistry,
    agent: String,
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        self.reg
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.agent);
        self.notify.notify_waiters();
    }
}

impl FlushRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a flush in progress for `agent`. Hold the guard for the flush
    /// duration; drop it when done (drop wakes waiters).
    pub fn begin(&self, agent: &str) -> FlushGuard {
        let notify = Arc::new(Notify::new());
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(agent.to_string(), notify.clone());
        FlushGuard { notify, reg: self.clone(), agent: agent.to_string() }
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
                Some(n) => n.clone(),
                None => return true, // idle → ready
            }
        };
        tokio::time::timeout(timeout, notify.notified()).await.is_ok()
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
        let h = tokio::spawn(async move {
            reg2.await_ready("main", Duration::from_secs(2)).await
        });
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
}
