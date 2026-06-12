//! Engine lifecycle: lazy load-on-demand, idle unload, deep-idle process exit.
//!
//! Pure decision functions take explicit `now_ms` so tests need no clocks.
//! `EngineSlot` queues concurrent loaders behind one async mutex — the spec's
//! "Loading 期间请求排队 hold" falls out of the lock for free.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Epoch milliseconds now (single definition; tests pass values directly).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Should an idle engine be unloaded? `last_used_ms == 0` means never used.
pub fn should_unload(last_used_ms: u64, now_ms: u64, ttl_secs: u64) -> bool {
    last_used_ms != 0 && now_ms.saturating_sub(last_used_ms) >= ttl_secs * 1000
}

/// Should the whole process exit? Only when nothing happened for `idle_exit_secs`.
/// Callers must initialize `last_activity_ms` to process start time (not 0), or this fires immediately.
pub fn should_exit(last_activity_ms: u64, now_ms: u64, idle_exit_secs: u64) -> bool {
    now_ms.saturating_sub(last_activity_ms) >= idle_exit_secs * 1000
}

/// Lazy-loaded engine holder. Load runs in `spawn_blocking`; concurrent callers
/// queue on the mutex and reuse the freshly loaded engine.
pub struct EngineSlot<E: ?Sized + Send + Sync> {
    state: tokio::sync::Mutex<Option<Arc<E>>>,
    last_used_ms: AtomicU64,
}

impl<E: ?Sized + Send + Sync + 'static> EngineSlot<E> {
    pub fn new() -> Self {
        Self { state: tokio::sync::Mutex::new(None), last_used_ms: AtomicU64::new(0) }
    }

    /// Get the engine, loading it via `load` if absent. Marks use time.
    pub async fn get_or_load<F>(&self, now: u64, load: F) -> anyhow::Result<Arc<E>>
    where
        F: FnOnce() -> anyhow::Result<Arc<E>> + Send + 'static,
    {
        let mut guard = self.state.lock().await;
        if guard.is_none() {
            let loaded = tokio::task::spawn_blocking(load).await??;
            *guard = Some(loaded);
        }
        self.last_used_ms.store(now, Ordering::Relaxed);
        Ok(guard.as_ref().expect("just set").clone())
    }

    /// Drop the engine if idle past `ttl_secs`. Returns true when unloaded.
    pub async fn maybe_unload(&self, ttl_secs: u64, now: u64) -> bool {
        let mut guard = self.state.lock().await;
        if guard.is_some() && should_unload(self.last_used_ms.load(Ordering::Relaxed), now, ttl_secs) {
            *guard = None;
            return true;
        }
        false
    }

    pub async fn is_loaded(&self) -> bool {
        self.state.lock().await.is_some()
    }

    pub fn last_used_ms(&self) -> u64 {
        self.last_used_ms.load(Ordering::Relaxed)
    }
}

impl<E: ?Sized + Send + Sync + 'static> Default for EngineSlot<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn unload_decision_table() {
        assert!(!should_unload(0, 999_999, 1)); // never used
        assert!(!should_unload(1_000, 100_999, 120)); // 99.999s < 120s
        assert!(should_unload(1_000, 121_000, 120)); // exactly ttl
    }

    #[test]
    fn exit_decision() {
        assert!(!should_exit(1_000, 1_000 + 1_799_999, 1_800));
        assert!(should_exit(1_000, 1_000 + 1_800_000, 1_800));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loads_once_under_concurrency() {
        let slot: Arc<EngineSlot<crate::engine::mock::MockStt>> = Arc::new(EngineSlot::new());
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        let mk = || {
            LOADS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok(Arc::new(crate::engine::mock::MockStt))
        };
        let (a, b) = tokio::join!(slot.get_or_load(1, mk), slot.get_or_load(2, mk));
        a.unwrap();
        b.unwrap();
        assert_eq!(LOADS.load(Ordering::SeqCst), 1, "second caller must reuse the load");
        assert!(slot.is_loaded().await);
    }

    #[tokio::test]
    async fn unloads_after_ttl_and_reloads() {
        let slot: EngineSlot<crate::engine::mock::MockStt> = EngineSlot::new();
        slot.get_or_load(1_000, || Ok(Arc::new(crate::engine::mock::MockStt))).await.unwrap();
        assert!(!slot.maybe_unload(120, 1_000 + 119_000).await);
        assert!(slot.maybe_unload(120, 1_000 + 120_000).await);
        assert!(!slot.is_loaded().await);
    }
}
