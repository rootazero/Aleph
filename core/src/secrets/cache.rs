//! TTL-based in-memory cache for resolved secrets.
//!
//! Provides a thread-safe cache that stores decrypted secrets with
//! per-entry time-to-live. Expired entries are invisible to readers
//! and can be bulk-evicted via [`SecretCache::evict_expired`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use super::types::DecryptedSecret;

/// A single cached entry with its own TTL.
struct CachedEntry {
    value: DecryptedSecret,
    fetched_at: Instant,
    ttl: Duration,
}

impl CachedEntry {
    /// Returns `true` if the entry has lived past its TTL.
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > self.ttl
    }
}

/// Thread-safe TTL-based in-memory cache for resolved secrets.
///
/// # Design notes
///
/// - `get()` returns `None` for both cache misses and expired entries.
///   Expired entries are **not** removed on read to keep the read path
///   lock-free (only a `RwLock` read guard is held).
/// - `evict_expired()` takes a write guard and sweeps all stale entries.
/// - Because `DecryptedSecret` does not implement `Clone`, the cache
///   constructs a fresh instance via `DecryptedSecret::new(expose())`.
pub struct SecretCache {
    entries: RwLock<HashMap<String, CachedEntry>>,
}

impl SecretCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Look up a secret by name.
    ///
    /// Returns `None` on cache miss **or** if the entry is expired.
    /// Expired entries are not removed here — call [`evict_expired`] for that.
    pub async fn get(&self, name: &str) -> Option<DecryptedSecret> {
        let entries = self.entries.read().await;
        let entry = entries.get(name)?;
        if entry.is_expired() {
            return None;
        }
        // DecryptedSecret doesn't impl Clone — rebuild from exposed value.
        Some(DecryptedSecret::new(entry.value.expose()))
    }

    /// Insert or overwrite a secret with a given TTL.
    ///
    /// `fetched_at` is set to `Instant::now()`.
    pub async fn put(&self, name: String, value: DecryptedSecret, ttl: Duration) {
        let entry = CachedEntry {
            value,
            fetched_at: Instant::now(),
            ttl,
        };
        let mut entries = self.entries.write().await;
        entries.insert(name, entry);
    }

    /// Remove a specific entry by name.
    pub async fn invalidate(&self, name: &str) {
        let mut entries = self.entries.write().await;
        entries.remove(name);
    }

    /// Remove all expired entries.
    pub async fn evict_expired(&self) {
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| !entry.is_expired());
    }

    /// Remove all entries.
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }
}

impl Default for SecretCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn put_and_get() {
        let cache = SecretCache::new();
        cache
            .put(
                "api_key".into(),
                DecryptedSecret::new("sk-123"),
                Duration::from_secs(60),
            )
            .await;

        let got = cache.get("api_key").await.expect("should hit cache");
        assert_eq!(got.expose(), "sk-123");
    }

    #[tokio::test]
    async fn cache_miss() {
        let cache = SecretCache::new();
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn cache_expiration() {
        let cache = SecretCache::new();
        cache
            .put(
                "ephemeral".into(),
                DecryptedSecret::new("short-lived"),
                Duration::from_millis(1),
            )
            .await;

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            cache.get("ephemeral").await.is_none(),
            "expired entry should return None"
        );
    }

    #[tokio::test]
    async fn cache_invalidate() {
        let cache = SecretCache::new();
        cache
            .put(
                "token".into(),
                DecryptedSecret::new("abc"),
                Duration::from_secs(60),
            )
            .await;

        cache.invalidate("token").await;
        assert!(
            cache.get("token").await.is_none(),
            "invalidated entry should return None"
        );
    }

    #[tokio::test]
    async fn cache_clear() {
        let cache = SecretCache::new();
        cache
            .put(
                "a".into(),
                DecryptedSecret::new("val_a"),
                Duration::from_secs(60),
            )
            .await;
        cache
            .put(
                "b".into(),
                DecryptedSecret::new("val_b"),
                Duration::from_secs(60),
            )
            .await;

        cache.clear().await;
        assert!(cache.get("a").await.is_none());
        assert!(cache.get("b").await.is_none());
    }

    #[tokio::test]
    async fn evict_expired() {
        let cache = SecretCache::new();

        // Short-lived entry (1ms TTL)
        cache
            .put(
                "short".into(),
                DecryptedSecret::new("gone-soon"),
                Duration::from_millis(1),
            )
            .await;

        // Long-lived entry (60s TTL)
        cache
            .put(
                "long".into(),
                DecryptedSecret::new("stays"),
                Duration::from_secs(60),
            )
            .await;

        tokio::time::sleep(Duration::from_millis(10)).await;
        cache.evict_expired().await;

        assert!(
            cache.get("short").await.is_none(),
            "short-lived entry should be evicted"
        );
        let long = cache.get("long").await.expect("long-lived entry should remain");
        assert_eq!(long.expose(), "stays");
    }
}
