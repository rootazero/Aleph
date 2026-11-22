//! RPC idempotency guard for preventing duplicate request execution.
//!
//! Uses `DashMap` for lock-free concurrent access. Tracks both completed
//! results (with TTL) and in-flight requests (via watch channels).

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Cache entry: in-flight or completed.
enum CacheEntry {
    /// Request is being processed; duplicates subscribe to this receiver.
    /// The `Instant` stamps when the slot was acquired so `prune` can evict a
    /// slot whose producer future was leaked (a task holding it across a hung
    /// dependency would otherwise pin the key as `InFlight` forever — every
    /// retry of that key then timed out for the process lifetime).
    InFlight(watch::Sender<Option<Value>>, Instant),
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
    sender: watch::Sender<Option<Value>>,
    guard: Option<std::sync::Arc<DashMap<String, CacheEntry>>>,
}

impl IdempotencySlot {
    /// Mark this slot as completed with a result. Consumes the guard.
    ///
    /// Uses `entry().and_modify()` for atomic InFlight→Complete transition,
    /// preventing a TOCTOU race where a concurrent request could see
    /// a vacant key between `remove()` and `insert()`.
    ///
    /// **Audit fix**: the previous implementation fell back to
    /// `entry().or_insert(Complete(...))` when `and_modify` reported the entry
    /// was gone (e.g. removed by `prune`, by another slot's `Drop`, or by a
    /// second `complete` call). That fallback re-created a `Complete` entry
    /// AFTER a fresh caller had already raced through `try_acquire`'s vacant
    /// arm and obtained a new `Proceed` slot — the original producer's stale
    /// `Complete` would then overwrite the in-flight slot of the next request,
    /// and waiters parked on the old `tx` would observe a result that no
    /// producer evaluated for the new request's key.
    ///
    /// Correct behavior: do NOT resurrect an entry that the cache has
    /// forgotten. A vanished entry means the slot's owner is gone and the
    /// next caller deserves a fresh `Proceed`, not the value from a cancelled
    /// run. The transition is therefore purely an `and_modify`; if it reports
    /// `Vacant`, the function returns silently.
    ///
    /// **G4-03 fix**: A stale slot (pruned after 2×TTL) can complete or discard
    /// its replacement if only the key is checked. Now we fence on acquisition
    /// identity: the slot stores the channel sender from its InFlight entry,
    /// and only modifies/removes entries whose sender matches. This prevents
    /// ABA races where the key is reused by a newer generation.
    pub fn complete(mut self, result: Value) {
        if let Some(cache) = self.guard.take() {
            let key = self.key.clone();
            // Atomic transition: InFlight → Complete (notify waiters in-place).
            // Only modify if the entry's sender matches ours (same generation).
            cache.entry(key).and_modify(|entry| {
                if let CacheEntry::InFlight(tx, _) = entry {
                    if tx.same_channel(&self.sender) {
                        let _ = tx.send(Some(result.clone()));
                        *entry = CacheEntry::Complete(result.clone(), Instant::now());
                    }
                }
            });
        }
    }

    /// Explicitly discard this slot (on error). Consumes the guard.
    pub fn discard(mut self) {
        self.do_discard();
    }

    fn do_discard(&mut self) {
        if let Some(cache) = self.guard.take() {
            cache.remove_if(&self.key, |_, entry| {
                if let CacheEntry::InFlight(tx, _) = entry {
                    if tx.same_channel(&self.sender) {
                        let _ = tx.send(None);
                        return true;
                    }
                }
                false
            });
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
    #[must_use]
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
    #[must_use]
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
                CacheEntry::InFlight(tx, _) => {
                    return AcquireResult::Waiting(tx.subscribe());
                }
            }
            drop(entry); // Explicitly drop before entry() to avoid deadlock
        }

        // Atomic check-and-insert via entry() API
        match self.cache.entry(key.to_string()) {
            Entry::Occupied(mut e) => {
                // Another thread beat us — re-check the entry. The shard lock
                // held by `e` must NOT be released before the expired-replace,
                // or two concurrent callers can both observe the same expired
                // Complete, both insert InFlight, and both get Proceed (a
                // TOCTOU duplicate-execution — the exact race this guard exists
                // to prevent). So decide while borrowing, then mutate in place.
                let cached = match e.get() {
                    CacheEntry::Complete(value, inserted_at) => {
                        if inserted_at.elapsed() < self.ttl {
                            Some(value.clone())
                        } else {
                            None
                        }
                    }
                    CacheEntry::InFlight(tx, _) => return AcquireResult::Waiting(tx.subscribe()),
                };
                if let Some(value) = cached {
                    return AcquireResult::Cached(value);
                }
                // Still expired — replace with InFlight without releasing the lock.
                let (tx, _rx) = watch::channel(None);
                *e.get_mut() = CacheEntry::InFlight(tx.clone(), Instant::now());
                AcquireResult::Proceed(IdempotencySlot {
                    key: key.to_string(),
                    sender: tx,
                    guard: Some(self.cache.clone()),
                })
            }
            Entry::Vacant(e) => {
                let (tx, _rx) = watch::channel(None);
                e.insert(CacheEntry::InFlight(tx.clone(), Instant::now()));
                AcquireResult::Proceed(IdempotencySlot {
                    key: key.to_string(),
                    sender: tx,
                    guard: Some(self.cache.clone()),
                })
            }
        }
    }

    /// Remove expired entries. Returns number of entries pruned.
    ///
    /// `Complete` entries are pruned at `ttl`. `InFlight` entries are pruned
    /// at `2 × ttl`: a healthy request never stays in flight that long, so a
    /// slot older than the bound belongs to a leaked producer future (the
    /// `IdempotencySlot` drop guard covers panics, but not a future parked
    /// forever on a hung dependency). Evicting it lets the next retry of the
    /// key acquire a fresh `Proceed` instead of timing out forever.
    #[must_use]
    pub fn prune(&self) -> usize {
        let inflight_ceiling = self.ttl.saturating_mul(2);
        let mut pruned = 0;
        self.cache.retain(|_, entry| match entry {
            CacheEntry::Complete(_, inserted_at) => {
                if inserted_at.elapsed() >= self.ttl {
                    pruned += 1;
                    false
                } else {
                    true
                }
            }
            CacheEntry::InFlight(_, started_at) => {
                if started_at.elapsed() >= inflight_ceiling {
                    pruned += 1;
                    false
                } else {
                    true
                }
            }
        });
        pruned
    }

    /// Number of entries currently in the cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_slots_cannot_complete_or_discard_a_replacement() {
        for action in ["complete", "discard", "drop"] {
            let guard = IdempotencyGuard::new(Duration::from_secs(10));
            let AcquireResult::Proceed(stale) = guard.try_acquire("key") else {
                panic!("first acquisition must proceed");
            };
            if let CacheEntry::InFlight(_, started) =
                guard.cache.get_mut("key").unwrap().value_mut()
            {
                *started = Instant::now() - Duration::from_secs(21);
            }
            assert_eq!(guard.prune(), 1);
            let AcquireResult::Proceed(current) = guard.try_acquire("key") else {
                panic!("replacement must proceed");
            };
            let AcquireResult::Waiting(rx) = guard.try_acquire("key") else {
                panic!("duplicate must wait");
            };
            match action {
                "complete" => stale.complete(serde_json::json!("stale")),
                "discard" => stale.discard(),
                _ => drop(stale),
            }
            assert!(matches!(
                guard.try_acquire("key"),
                AcquireResult::Waiting(_)
            ));
            assert_eq!(*rx.borrow(), None);
            current.complete(serde_json::json!("current"));
            assert_eq!(*rx.borrow(), Some(serde_json::json!("current")));
        }
    }

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
