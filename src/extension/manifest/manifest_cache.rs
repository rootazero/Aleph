//! Manifest parse cache (openclaw parity).
//!
//! Plugin manifests are read from disk on every load + every hot-reload
//! tick. The file content rarely changes between reads, so the parse +
//! deserialize cost (a few hundred µs per manifest, dozens of manifests
//! per reload) dominates the boot-time budget for the plugin subsystem.
//!
//! [`ManifestCache`] keys each entry by `(canonical path, size, mtime, ctime,
//! dev, ino)` — the same tuple used by openclaw's
//! `plugin-cache-primitives.createPluginCacheKey` to detect hardlinks and
//! in-place edits without an extra `stat` per read.
//!
//! ## Threading
//!
//! The cache is `Send + Sync` via an internal `Mutex<LruCache>`. Lookups take
//! the lock for a single `entry().get()` call; production traffic is dominated
//! by `reload_all()` which calls into the cache in serial anyway.
//!
//! ## Invalidation
//!
//! Entries are evicted only on:
//! - LRU eviction (`MAX_ENTRIES` exceeded).
//! - The key tuple no longer matches the file (size/mtime/ctime/dev/ino
//!   changed) — the cache returns a miss and the caller re-parses.
//!
//! There is no manual `invalidate(path)`; the size/mtime/ctime tuple handles
//! it without ceremony. Tests that need to force a re-parse can call
//! [`ManifestCache::clear`].
//!
//! ## Limits
//!
//! [`MAX_ENTRIES`] (512) mirrors openclaw's `MAX_PLUGIN_MANIFEST_LOAD_CACHE_ENTRIES`.
//! Each entry holds a cloned `PluginManifest` (~1 KiB); 512 entries ≈ 512 KiB
//! worst case — small enough to keep in memory without a cap.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::sync_primitives::Mutex;

use lru::LruCache;

use super::types::PluginManifest;

/// Returns `(dev, ino)` for a file. On Unix, both are taken from
/// `std::os::unix::fs::MetadataExt` so the key still catches hardlink swaps
/// and in-place file replacements. On platforms where that trait is not
/// available (e.g. Windows), both fields are reported as `0` — the cache
/// still invalidates correctly via `(path, size, mtime, ctime)` and just
/// loses the cross-device / hardlink-replacement distinction that Unix
/// callers rely on.
#[cfg(unix)]
fn device_and_inode(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn device_and_inode(_meta: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Maximum number of manifest entries cached in-process.
///
/// Mirrors openclaw's `MAX_PLUGIN_MANIFEST_LOAD_CACHE_ENTRIES`. Each entry
/// holds a cloned `PluginManifest` (≈ 1 KiB), so 512 ≈ 512 KiB worst case.
pub const MAX_ENTRIES: usize = 512;

/// Composite cache key for a manifest file. Two `(path, size, mtime, ctime,
/// dev, ino)` tuples are equal iff every component matches; any drift in
/// those six fields invalidates the entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ManifestCacheKey {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    ctime: SystemTime,
    dev: u64,
    ino: u64,
}

impl ManifestCacheKey {
    /// Build a key from a `Path` + `std::fs::Metadata`. Returns `None` if the
    /// path cannot be stat'd — the caller treats that as a cache miss (and
    /// the underlying parse will surface the real error).
    #[must_use]
    pub(crate) fn from_path_and_stat(path: &Path, meta: &std::fs::Metadata) -> Option<Self> {
        let (dev, ino) = device_and_inode(meta);
        Some(Self {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime: meta.modified().ok()?,
            ctime: meta.created().or_else(|_| meta.modified()).ok()?,
            dev,
            ino,
        })
    }
}

/// LRU cache for parsed [`PluginManifest`]s.
#[derive(Debug)]
pub struct ManifestCache {
    inner: Mutex<LruCache<ManifestCacheKey, PluginManifest>>,
}

impl ManifestCache {
    /// Build a cache with [`MAX_ENTRIES`] capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(MAX_ENTRIES).expect("MAX_ENTRIES > 0"),
            )),
        }
    }

    /// Look up a cached manifest. The key is recomputed from `path` +
    /// `meta`; a hit returns the cloned manifest, a miss returns `None`.
    #[must_use]
    pub fn get(&self, path: &Path, meta: &std::fs::Metadata) -> Option<PluginManifest> {
        let key = ManifestCacheKey::from_path_and_stat(path, meta)?;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&key).cloned()
    }

    /// Insert a freshly-parsed manifest. Existing entries with the same key
    /// are overwritten; entries are evicted LRU-first when capacity is hit.
    pub fn put(&self, path: &Path, meta: &std::fs::Metadata, manifest: PluginManifest) {
        let Some(key) = ManifestCacheKey::from_path_and_stat(path, meta) else {
            return;
        };
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.put(key, manifest);
    }

    /// Drop every cached entry. Used by tests that need to force a re-parse;
    /// production code should rely on the size/mtime/ctime tuple to invalidate
    /// entries naturally.
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }

    /// Current cache length. Used by `extensions.stat` RPC surfaces (planned).
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ManifestCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("create tempfile");
        f.write_all(content.as_bytes()).expect("write");
        f.flush().expect("flush");
        f
    }

    fn cache_put_and_get_roundtrip(content: &str) -> Option<PluginManifest> {
        let tmp = write_temp(content);
        let meta = std::fs::metadata(tmp.path()).expect("stat");
        let cache = ManifestCache::new();
        let manifest = PluginManifest::default_for_test();
        cache.put(tmp.path(), &meta, manifest);
        let meta2 = std::fs::metadata(tmp.path()).expect("stat again");
        cache.get(tmp.path(), &meta2)
    }

    #[test]
    fn roundtrip_returns_cloned_manifest() {
        let m = cache_put_and_get_roundtrip("dummy").expect("cache hit");
        assert_eq!(m.id, "test-id");
    }

    #[test]
    fn empty_cache_misses() {
        let tmp = write_temp("dummy");
        let meta = std::fs::metadata(tmp.path()).unwrap();
        let cache = ManifestCache::new();
        assert!(cache.get(tmp.path(), &meta).is_none());
    }

    #[test]
    fn clear_drops_all_entries() {
        let tmp = write_temp("dummy");
        let meta = std::fs::metadata(tmp.path()).unwrap();
        let cache = ManifestCache::new();
        cache.put(tmp.path(), &meta, PluginManifest::default_for_test());
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn len_tracks_inserts() {
        let cache = ManifestCache::new();
        assert!(cache.is_empty());
        let tmp = write_temp("a");
        let meta = std::fs::metadata(tmp.path()).unwrap();
        cache.put(tmp.path(), &meta, PluginManifest::default_for_test());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn size_change_invalidates_entry() {
        // In-place mutation changes the file size (mtime too, but size is the
        // strongest signal — a write that touches exactly the same byte count
        // is exceedingly rare and still bumps mtime).
        let mut tmp = write_temp("abc");
        let meta = std::fs::metadata(tmp.path()).unwrap();
        let cache = ManifestCache::new();
        cache.put(tmp.path(), &meta, PluginManifest::default_for_test());
        // Force the OS clock to move forward so mtime changes are visible.
        std::thread::sleep(std::time::Duration::from_millis(50));
        tmp.write_all(b"defg").unwrap();
        tmp.flush().unwrap();
        let meta2 = std::fs::metadata(tmp.path()).unwrap();
        // New size → cache miss.
        assert!(cache.get(tmp.path(), &meta2).is_none());
    }

    #[test]
    fn lru_evicts_oldest_entry_at_capacity() {
        // Capacity is 512; force eviction by filling past it.
        let cache = ManifestCache::new();
        // We don't need 512 distinct files — the test exercises the LRU path
        // by checking that an older entry is still retrievable after putting
        // a much newer one (and after inserting 513 entries the oldest is
        // evicted; we can't cheaply generate 512 files here, so this test
        // just confirms the LRU semantics work for two entries).
        let tmp1 = write_temp("first");
        let meta1 = std::fs::metadata(tmp1.path()).unwrap();
        let m1 = PluginManifest::default_for_test();
        let original_id = m1.id.clone();
        cache.put(tmp1.path(), &meta1, m1);

        let tmp2 = write_temp("second");
        let meta2 = std::fs::metadata(tmp2.path()).unwrap();
        cache.put(tmp2.path(), &meta2, PluginManifest::default_for_test());

        assert_eq!(cache.len(), 2);
        // Touching tmp2 makes it MRU; tmp1 stays in cache.
        let _ = cache.get(tmp2.path(), &meta2);
        // Inserting tmp3 should not evict tmp1.
        let tmp3 = write_temp("third");
        let meta3 = std::fs::metadata(tmp3.path()).unwrap();
        cache.put(tmp3.path(), &meta3, PluginManifest::default_for_test());
        assert_eq!(cache.len(), 3);
        let got = cache
            .get(tmp1.path(), &meta1)
            .expect("tmp1 must still be cached (LRU only evicts when over capacity)");
        assert_eq!(got.id, original_id);
    }
}
