//! Disk persistence for ACP sessions.
//!
//! Format: a JSON array of [`crate::acp::session::PersistedAcpSession`] entries
//! at `~/.aleph/data/acp_sessions.json`. Best-effort — parse failures fall back
//! to "no persisted sessions" and write failures only warn.

use tracing::{info, warn};

use super::AcpAdapterManager;
use crate::sync_primitives::Arc;

/// Default persistence file path for ACP sessions.
fn acp_sessions_path() -> std::path::PathBuf {
    // `aleph_protocol::paths::data_dir` rather than this crate's `get_data_dir`:
    // the loader must be able to ask where the file *would* be without bringing
    // the directory into existence. The writer below creates it.
    aleph_protocol::paths::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".").join(".aleph").join("data"))
        .join("acp_sessions.json")
}

/// Load persisted ACP sessions from disk (best-effort).
#[must_use]
pub fn load_persisted_sessions() -> Vec<crate::acp::session::PersistedAcpSession> {
    let path = acp_sessions_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!("Failed to parse ACP sessions file: {}", e);
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Save persisted ACP sessions to disk (atomic write).
pub fn save_persisted_sessions(sessions: &[crate::acp::session::PersistedAcpSession]) {
    let path = acp_sessions_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("Failed to create ACP sessions directory: {}", e);
        }
    }
    match serde_json::to_string_pretty(sessions) {
        Ok(json) => {
            if let Err(e) = crate::utils::atomic_io::write_atomic(&path, json.as_bytes()) {
                warn!("Failed to atomic-write ACP sessions: {}", e);
            }
        }
        Err(e) => warn!("Failed to serialize ACP sessions: {}", e),
    }
}

/// Wire up file-based persistence for ACP sessions.
/// Call this after creating the `AcpAdapterManager` at startup.
/// Apply an [`AcpSessionEvent`] to the in-memory session store.
///
/// Pulled out of [`wire_persistence`] so the persistence worker can call it
/// in a single critical section (lock → mutate → snapshot → unlock),
/// avoiding the stale-snapshot race that arose when each event captured
/// its own snapshot mid-flight.
///
/// `Created` is re-emitted idempotently after every prompt, so the
/// `created_at` of the existing record is preserved; only `last_used_at`
/// moves. `Removed` drops the matching entry.
fn apply_event_to_store(
    store: &mut Vec<crate::acp::session::PersistedAcpSession>,
    event: &crate::acp::AcpSessionEvent,
) {
    use crate::acp::AcpSessionEvent;
    match event {
        AcpSessionEvent::Created {
            harness_id,
            acp_session_id,
            cwd,
            session_name,
        } => {
            // Match the full triple so a `backend` name doesn't displace the
            // unnamed/default entry under the same cwd.
            let prior_created = store
                .iter()
                .find(|s| {
                    s.harness_id == *harness_id
                        && s.cwd == *cwd
                        && s.session_name == *session_name
                })
                .map(|s| s.created_at);
            store.retain(|s| {
                !(s.harness_id == *harness_id
                    && s.cwd == *cwd
                    && s.session_name == *session_name)
            });
            let now = chrono::Utc::now();
            store.push(crate::acp::session::PersistedAcpSession {
                harness_id: harness_id.clone(),
                acp_session_id: acp_session_id.clone(),
                cwd: cwd.clone(),
                created_at: prior_created.unwrap_or(now),
                last_used_at: now,
                session_name: session_name.clone(),
            });
        }
        AcpSessionEvent::Removed {
            harness_id,
            cwd,
            session_name,
        } => {
            store.retain(|s| {
                !(s.harness_id == *harness_id
                    && s.cwd == *cwd
                    && s.session_name == *session_name)
            });
        }
    }
}

/// Wire up file-based persistence for ACP sessions.
/// Call this after creating the `AcpAdapterManager` at startup.
///
/// Returns the channel sender the hook uses to forward events to the
/// persistence worker. Production callers can ignore the return value
/// (`let _ = wire_persistence(&manager).await;`); tests and migration
/// tools can drive events through the returned sender directly.
///
/// ## Stale-snapshot race fix (wire-persistence-race)
///
/// The previous implementation spawned a fresh `tokio::task` per event,
/// each capturing its own `store.clone()` snapshot at hook time. The
/// `file_lock` only serialized the *rename* step, not the snapshot
/// capture — so two concurrent writers could land on disk in
/// capture-time order, not event order: an older snapshot could clobber
/// a newer one if it grabbed the file lock last.
///
/// This implementation funnels every event through an `mpsc::unbounded_channel`
/// into a single persistence worker. The worker applies the event, takes
/// the latest snapshot under the same lock release, then persists it.
/// Events are processed strictly in arrival order, and the on-disk file
/// is always the result of applying every prior event.
///
/// The advisory `fs2` lock is retained as belt-and-suspenders: in the
/// pathological case where the worker task itself dies and a replacement
/// process is started mid-write, the lock keeps external observers from
/// reading a half-written `acp_sessions.json`.
pub async fn wire_persistence(
    manager: &AcpAdapterManager,
) -> tokio::sync::mpsc::UnboundedSender<crate::acp::AcpSessionEvent> {
    use crate::sync_primitives::Mutex;

    let sessions = Arc::new(Mutex::new(
        tokio::task::spawn_blocking(load_persisted_sessions)
            .await
            .unwrap_or_else(|e| {
                warn!("ACP session load task failed: {}", e);
                Vec::new()
            }),
    ));

    // The hook closure needs to `move` its sender, but the returned
    // sender must outlive the closure. Wrap the underlying sender in
    // `Arc` so both the hook and the public return value are cheap
    // clones of the same channel endpoint.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::acp::AcpSessionEvent>();
    let tx = Arc::new(tx);

    // Single persistence worker. All events flow through this task
    // serially, so every disk write reflects the cumulative state of
    // every prior event — the race that existed when each event
    // spawned its own task is gone.
    let worker_sessions = Arc::clone(&sessions);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            // 1. Apply event to in-memory store (under mutex) and snapshot
            //    the post-application state in the SAME critical section.
            //    No concurrent writer exists — this worker is the only writer.
            let snapshot = {
                let mut store = worker_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                apply_event_to_store(&mut store, &event);
                store.clone()
            };

            // 2. Persist the just-updated snapshot under the file lock. The
            //    lock is now strictly an external-collision safeguard — within
            //    this worker there is exactly one concurrent writer (itself).
            if let Err(e) = tokio::task::spawn_blocking(move || {
                let lock_path = crate::acp::manager::persistence::acp_sessions_path()
                    .with_extension("json.lock");
                crate::utils::atomic_io::with_file_lock(&lock_path, |_guard| {
                    save_persisted_sessions(&snapshot);
                    Ok(())
                })
            })
            .await
            {
                tracing::warn!(?e, "persist_sessions: spawn_blocking failed");
            }
        }
    });

    // Hook is now a pure channel sender: no locking, no spawning, no
    // snapshot capture. The cost per event is one bounded `unbounded_send`.
    let hook_tx = Arc::clone(&tx);
    manager
        .set_persistence_hook(Arc::new(move |event| {
            // `send` only fails if the worker task has exited (e.g.
            // runtime shutdown). Drop the event in that case — losing
            // the on-disk write is strictly better than blocking the
            // caller on a dead channel, and the in-memory state already
            // reflects the event.
            let _ = hook_tx.send(event);
        }))
        .await;

    // Restore existing sessions
    let persisted = sessions.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !persisted.is_empty() {
        let restored = manager.restore_sessions(persisted).await;
        if !restored.is_empty() {
            info!(count = restored.len(), "Restored ACP sessions from disk");
        }
    }

    // Unwrap the Arc if it's the last clone, otherwise clone the sender.
    // Returning `UnboundedSender` (not `Arc<UnboundedSender>`) keeps the
    // public API ergonomic for callers.
    Arc::try_unwrap(tx).unwrap_or_else(|arc| (*arc).clone())
}

#[cfg(test)]
mod wire_persistence_tests {
    //! Concurrency tests for the persistence worker.
    //!
    //! These tests construct an `AcpAdapterManager`, wire persistence against
    //! a temp directory (so we don't touch the real `~/.aleph/data/` file),
    //! fire a burst of `Created`/`Removed` events through the returned
    //! sender, and verify the on-disk file reflects the cumulative state —
    //! i.e. that the stale-snapshot race is dead.

    use super::*;
    use crate::acp::manager::AcpAdapterManager;

    /// Override `acp_sessions_path()` for the duration of `f` by setting
    /// `ALEPH_DATA_DIR`. Returns the temp dir; callers MUST keep it alive
    /// until `f` returns.
    fn with_temp_data_dir<F: FnOnce(&std::path::Path)>(f: F) {
        let tmp = tempfile::tempdir().expect("tempdir");
        // SAFETY: tests in this module run sequentially under cargo's
        // default test harness, and we set this before constructing the
        // manager and restore on drop.
        unsafe { std::env::set_var("ALEPH_DATA_DIR", tmp.path()) };
        f(tmp.path());
        unsafe { std::env::remove_var("ALEPH_DATA_DIR") };
    }

    fn mk_event_created(i: usize, suffix: &str) -> crate::acp::AcpSessionEvent {
        crate::acp::AcpSessionEvent::Created {
            harness_id: format!("h-{i}"),
            acp_session_id: format!("s-{i}{suffix}"),
            cwd: format!("/tmp/cwd-{i}"),
            session_name: None,
        }
    }

    fn mk_event_removed(i: usize) -> crate::acp::AcpSessionEvent {
        crate::acp::AcpSessionEvent::Removed {
            harness_id: format!("h-{i}"),
            cwd: format!("/tmp/cwd-{i}"),
            session_name: None,
        }
    }

    /// Fire a burst of Created events from N tasks concurrently. The
    /// final file must contain every (harness_id, cwd) tuple exactly once,
    /// proving no event was lost to the stale-snapshot race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn burst_of_creates_no_event_lost() {
        with_temp_data_dir(|_| {
            let manager = AcpAdapterManager::new();
            let tx = wire_persistence(&manager).await;

            const N: usize = 200;
            let mut handles = Vec::with_capacity(N);
            for i in 0..N {
                let tx = tx.clone();
                let h = tokio::spawn(async move {
                    let _ = tx.send(mk_event_created(i, ""));
                });
                handles.push(h);
            }
            for h in handles {
                h.await.expect("task join");
            }
            // Drain
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            let on_disk = load_persisted_sessions();
            assert_eq!(
                on_disk.len(),
                N,
                "expected {N} entries on disk after burst, got {} — stale-snapshot race?",
                on_disk.len()
            );
            let mut seen: std::collections::HashSet<(String, String)> =
                std::collections::HashSet::new();
            for entry in &on_disk {
                let key = (entry.harness_id.clone(), entry.cwd.clone());
                assert!(seen.insert(key.clone()), "duplicate entry {key:?}");
            }
        });
    }

    /// Interleave Created + Removed for the same key. The final on-disk
    /// state must show empty — never a Created that was supposed to be
    /// cancelled by a later Removed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn interleaved_create_remove_no_cancelled_create() {
        with_temp_data_dir(|_| {
            let manager = AcpAdapterManager::new();
            let tx = wire_persistence(&manager).await;

            const N: usize = 100;
            for i in 0..N {
                let _ = tx.send(mk_event_created(i, ""));
                let _ = tx.send(mk_event_removed(i));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            let on_disk = load_persisted_sessions();
            assert!(
                on_disk.is_empty(),
                "after {N} Created+Removed pairs the store must be empty, \
                 found {} entries — Removed lost to stale-snapshot race?",
                on_disk.len()
            );
        });
    }

    /// Sanity: the worker preserves `created_at` across repeated Created
    /// events for the same triple (idempotent re-emit after every prompt).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_created_preserves_created_at() {
        with_temp_data_dir(|_| {
            let manager = AcpAdapterManager::new();
            let tx = wire_persistence(&manager).await;

            let before = chrono::Utc::now();
            for _ in 0..5 {
                let _ = tx.send(mk_event_created(7, ""));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let after = chrono::Utc::now();

            let on_disk = load_persisted_sessions();
            assert_eq!(on_disk.len(), 1, "exactly one entry expected");
            let entry = &on_disk[0];
            assert!(
                entry.created_at <= after && entry.created_at >= before,
                "created_at {} must lie within [{}, {}]",
                entry.created_at, before, after,
            );
        });
    }
}

#[cfg(test)]
pub(super) fn acp_sessions_path_for_test() -> std::path::PathBuf {
    acp_sessions_path()
}
