//! Process-wide per-path write serialization for file-mutating tools.
//!
//! The harness's parallel fast path already prevents same-path races *within
//! one tool batch* via resource-scope claims ([`crate::tools::concurrency`]).
//! That guard cannot see across harness instances: a parent agent and a
//! concurrent subagent (or two team members running without worktree
//! isolation) share the same workspace, and `file_edit`'s read → locate →
//! apply → write critical section is a classic lost-update window — the
//! atomic temp-file rename only prevents *torn* writes, not overlapping
//! read-modify-write cycles.
//!
//! This module is the cross-agent guard: a process-global map of per-path
//! async mutexes, keyed by the already-canonicalized path the tools resolve
//! through `check_and_resolve_path`. Mutating tools acquire the path lock
//! for the duration of their critical section; reads stay lock-free.
//!
//! Same role as pi's per-file promise chain (`file-mutation-queue.ts`) and
//! hermes-agent's per-path `threading.Lock` in `file_state.py`, with the
//! map pruned opportunistically: entries whose `Arc` is held only by the
//! map itself (no guard or in-flight waiter) are dropped on each acquire,
//! so the map stays bounded by the number of *concurrently* contended paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;

use crate::sync_primitives::{Arc, Mutex};

static PATH_LOCKS: Lazy<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Acquire the write lock for `path`, waiting if another task holds it.
///
/// `path` should be the canonical path returned by `check_and_resolve_path`
/// so that two spellings of the same file map to one lock. The returned
/// owned guard keeps the per-path mutex alive; dropping it releases the
/// lock and makes the entry eligible for pruning.
pub async fn lock_path(path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    let cell = {
        let mut map = PATH_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        // Opportunistic prune: strong_count == 1 means only the map holds
        // the Arc — no guard outstanding, no waiter mid-acquire.
        map.retain(|_, m| Arc::strong_count(m) > 1);
        map.entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    cell.lock_owned().await
}

/// Acquire the write locks for a two-endpoint mutation (move / copy source +
/// destination) in **sorted order**, so two concurrent operations with
/// crossed endpoints (A→B racing B→A) cannot ABBA-deadlock — the same
/// discipline `apply_patch` follows for its move destinations. Equal paths
/// lock once (the second slot stays `None`); re-acquiring the same async
/// mutex in one task would deadlock, not panic.
pub async fn lock_path_pair(
    a: &Path,
    b: &Path,
) -> (
    tokio::sync::OwnedMutexGuard<()>,
    Option<tokio::sync::OwnedMutexGuard<()>>,
) {
    if a == b {
        return (lock_path(a).await, None);
    }
    let (first, second) = if a < b { (a, b) } else { (b, a) };
    let g1 = lock_path(first).await;
    let g2 = lock_path(second).await;
    (g1, Some(g2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn same_path_serializes() {
        static IN_CRITICAL: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from("/tmp/aleph-path-lock-test-same");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let p = path.clone();
            handles.push(tokio::spawn(async move {
                let _g = lock_path(&p).await;
                let now = IN_CRITICAL.fetch_add(1, Ordering::SeqCst);
                assert_eq!(now, 0, "two tasks inside the same-path critical section");
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                IN_CRITICAL.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("task");
        }
    }

    #[tokio::test]
    async fn different_paths_do_not_block_each_other() {
        let a = PathBuf::from("/tmp/aleph-path-lock-test-a");
        let b = PathBuf::from("/tmp/aleph-path-lock-test-b");
        let _ga = lock_path(&a).await;
        // Must complete immediately even while `a` is held.
        let gb = tokio::time::timeout(std::time::Duration::from_secs(1), lock_path(&b))
            .await
            .expect("disjoint path lock must not wait");
        drop(gb);
    }

    #[tokio::test]
    async fn entries_are_pruned_after_release() {
        let path = PathBuf::from("/tmp/aleph-path-lock-test-prune");
        let g = lock_path(&path).await;
        drop(g);
        // A later acquire on a different path triggers the prune sweep.
        let other = PathBuf::from("/tmp/aleph-path-lock-test-prune-other");
        let g2 = lock_path(&other).await;
        {
            let map = PATH_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
            assert!(
                !map.contains_key(&path),
                "released entry must be pruned on the next acquire"
            );
        }
        drop(g2);
    }
}
