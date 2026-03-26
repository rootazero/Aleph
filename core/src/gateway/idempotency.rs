//! RPC idempotency guard for preventing duplicate request execution.
//!
//! Uses DashMap for lock-free concurrent access. Tracks both completed
//! results (with TTL) and in-flight requests (via watch channels).

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Cache entry: in-flight or completed.
enum CacheEntry {
    /// Request is being processed; duplicates subscribe to this receiver.
    InFlight(watch::Sender<Option<Value>>),
    /// Result cached until TTL expires.
    Complete(Value, Instant),
}

/// Result of trying to acquire an idempotency slot.
pub enum AcquireResult {
    /// First request — caller should execute and call `complete()`.
    /// Wraps a drop guard that auto-discards if not explicitly completed.
    Proceed(IdempotencySlot),
    /// Cached result available — return immediately.
    Cached(Value),
    /// Another request with this key is in-flight — await the receiver.
    Waiting(watch::Receiver<Option<Value>>),
}

/// RAII guard for an acquired idempotency slot.
/// If dropped without calling `complete()`, automatically discards
/// the entry so the next request can retry (P7: Defensive Design).
pub struct IdempotencySlot {
    key: String,
    guard: Option<std::sync::Arc<DashMap<String, CacheEntry>>>,
}

impl IdempotencySlot {
    /// Mark this slot as completed with a result. Consumes the guard.
    ///
    /// Uses entry().and_modify() for atomic InFlight→Complete transition,
    /// preventing a TOCTOU race where a concurrent request could see
    /// a vacant key between remove() and insert().
    pub fn complete(mut self, result: Value) {
        if let Some(cache) = self.guard.take() {
            let key = self.key.clone();
            let mut notified = false;

            // Atomic transition: InFlight → Complete (notify waiters in-place)
            cache.entry(key.clone()).and_modify(|entry| {
                if let CacheEntry::InFlight(tx) = entry {
                    let _ = tx.send(Some(result.clone()));
                    notified = true;
                }
                *entry = CacheEntry::Complete(result.clone(), Instant::now());
            });

            // If entry was already removed (e.g., expired by prune), insert fresh
            if !notified {
                cache.entry(key).or_insert(CacheEntry::Complete(result, Instant::now()));
            }
        }
    }

    /// Explicitly discard this slot (on error). Consumes the guard.
    pub fn discard(mut self) {
        self.do_discard();
    }

    fn do_discard(&mut self) {
        if let Some(cache) = self.guard.take() {
            if let Some((_, entry)) = cache.remove(&self.key) {
                if let CacheEntry::InFlight(tx) = entry {
                    let _ = tx.send(None);
                }
            }
        }
    }
}

impl Drop for IdempotencySlot {
    fn drop(&mut self) {
        // Auto-discard if not explicitly completed (e.g., panic)
        self.do_discard();
    }
}

/// Lock-free RPC idempotency guard with TTL-based expiry.
pub struct IdempotencyGuard {
    cache: std::sync::Arc<DashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl IdempotencyGuard {
    /// Create a new guard with the given TTL for cached results.
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: std::sync::Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Try to acquire an idempotency slot for the given key.
    ///
    /// Uses `DashMap::entry()` for atomic check-and-insert to prevent
    /// TOCTOU race conditions between concurrent requests.
    pub fn try_acquire(&self, key: &str) -> AcquireResult {
        // First, check for existing non-expired entries without holding an entry lock
        // (entry() on an occupied key would block other readers of the same key)
        if let Some(entry) = self.cache.get(key) {
            match entry.value() {
                CacheEntry::Complete(value, inserted_at) => {
                    if inserted_at.elapsed() < self.ttl {
                        return AcquireResult::Cached(value.clone());
                    }
                    // Expired — drop ref, fall through to entry() below
                }
                CacheEntry::InFlight(tx) => {
                    return AcquireResult::Waiting(tx.subscribe());
                }
            }
            drop(entry); // Explicitly drop before entry() to avoid deadlock
        }

        // Atomic check-and-insert via entry() API
        match self.cache.entry(key.to_string()) {
            Entry::Occupied(e) => {
                // Another thread beat us — re-check the entry
                match e.get() {
                    CacheEntry::Complete(value, inserted_at) => {
                        if inserted_at.elapsed() < self.ttl {
                            return AcquireResult::Cached(value.clone());
                        }
                        // Still expired — replace with InFlight
                        drop(e);
                        let (tx, _rx) = watch::channel(None);
                        self.cache.insert(key.to_string(), CacheEntry::InFlight(tx));
                        AcquireResult::Proceed(IdempotencySlot {
                            key: key.to_string(),
                            guard: Some(self.cache.clone()),
                        })
                    }
                    CacheEntry::InFlight(tx) => {
                        AcquireResult::Waiting(tx.subscribe())
                    }
                }
            }
            Entry::Vacant(e) => {
                let (tx, _rx) = watch::channel(None);
                e.insert(CacheEntry::InFlight(tx));
                AcquireResult::Proceed(IdempotencySlot {
                    key: key.to_string(),
                    guard: Some(self.cache.clone()),
                })
            }
        }
    }

    /// Remove expired entries. Returns number of entries pruned.
    pub fn prune(&self) -> usize {
        let mut pruned = 0;
        self.cache.retain(|_, entry| {
            match entry {
                CacheEntry::Complete(_, inserted_at) => {
                    if inserted_at.elapsed() >= self.ttl {
                        pruned += 1;
                        false
                    } else {
                        true
                    }
                }
                CacheEntry::InFlight(_) => true, // Keep in-flight entries
            }
        });
        pruned
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_request_proceeds() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_slot) => {} // expected
            _ => panic!("First request should proceed"),
        }
    }

    #[test]
    fn test_duplicate_gets_waiting() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        // First request — hold the slot to keep it InFlight
        let _slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Second request with same key — should wait
        match guard.try_acquire("key-1") {
            AcquireResult::Waiting(_) => {} // expected
            _ => panic!("Duplicate request should wait"),
        }
    }

    #[test]
    fn test_completed_returns_cached() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        // First request proceeds
        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Complete it via the slot
        slot.complete(serde_json::json!({"result": "ok"}));

        // Next request with same key — should get cached
        match guard.try_acquire("key-1") {
            AcquireResult::Cached(val) => {
                assert_eq!(val, serde_json::json!({"result": "ok"}));
            }
            _ => panic!("Should return cached result"),
        }
    }

    #[tokio::test]
    async fn test_waiter_receives_result() {
        let guard = std::sync::Arc::new(IdempotencyGuard::new(Duration::from_secs(300)));

        // First request proceeds
        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("First request should proceed"),
        };

        // Second request waits
        let mut rx = match guard.try_acquire("key-1") {
            AcquireResult::Waiting(rx) => rx,
            _ => panic!("Should be waiting"),
        };

        // Complete from another "thread" via the slot
        tokio::spawn(async move {
            slot.complete(serde_json::json!(42));
        });

        // Waiter should receive the result
        rx.changed().await.unwrap();
        let val = rx.borrow().clone();
        assert_eq!(val, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_discard_allows_retry() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };

        // Explicitly discard (simulating an error)
        slot.discard();

        // Next request should proceed (not wait or return cached)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected — can retry
            _ => panic!("After discard, should be able to proceed"),
        }
    }

    #[test]
    fn test_drop_guard_auto_discards() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        {
            let _slot = match guard.try_acquire("key-1") {
                AcquireResult::Proceed(slot) => slot,
                _ => panic!("Should proceed"),
            };
            // _slot dropped here without complete() — auto-discards
        }

        // Next request should proceed (auto-discarded)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected
            _ => panic!("After auto-discard, should be able to proceed"),
        }
    }

    #[test]
    fn test_prune_removes_expired() {
        let guard = IdempotencyGuard::new(Duration::from_millis(1));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };
        slot.complete(serde_json::json!("done"));

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        let pruned = guard.prune();
        assert_eq!(pruned, 1);
        assert!(guard.is_empty());
    }

    #[test]
    fn test_expired_entry_allows_new_request() {
        let guard = IdempotencyGuard::new(Duration::from_millis(1));

        let slot = match guard.try_acquire("key-1") {
            AcquireResult::Proceed(slot) => slot,
            _ => panic!("Should proceed"),
        };
        slot.complete(serde_json::json!("old"));

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        // New request should proceed (expired)
        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {} // expected — expired entry replaced
            _ => panic!("Expired entry should allow new request"),
        }
    }

    #[test]
    fn test_different_keys_independent() {
        let guard = IdempotencyGuard::new(Duration::from_secs(300));

        match guard.try_acquire("key-1") {
            AcquireResult::Proceed(_) => {}
            _ => panic!("key-1 should proceed"),
        }

        match guard.try_acquire("key-2") {
            AcquireResult::Proceed(_) => {} // different key, independent
            _ => panic!("key-2 should proceed independently"),
        }
    }
}
