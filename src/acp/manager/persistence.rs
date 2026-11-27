//! Disk persistence for ACP sessions.
//!
//! Format: a JSON array of [`crate::acp::session::PersistedAcpSession`] entries
//! at `~/.aleph/data/acp_sessions.json`. Best-effort — parse failures fall back
//! to "no persisted sessions" and write failures warn and are reported when the writer closes.
//!
//! **Projection-only boundary:** this JSON is a re-creatable protocol/session
//! projection. It is not an authorization source, capability registry, or
//! recovery source of truth.

use tracing::warn;

use crate::error::{AlephError, Result};

use super::{AcpAdapterManager, SessionKey};
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
    if let Err(error) = write_sessions(sessions) {
        warn!(%error, "Failed to save ACP sessions");
    }
}

fn write_sessions(sessions: &[crate::acp::session::PersistedAcpSession]) -> Result<()> {
    let path = acp_sessions_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(sessions)
        .map_err(|error| AlephError::other(format!("Failed to serialize ACP sessions: {error}")))?;
    crate::utils::atomic_io::write_atomic(&path, json.as_bytes())?;
    Ok(())
}

/// Apply an event to the store owned exclusively by the serial writer.
/// Events are applied and written in receive order to avoid stale snapshots.
///
/// `Created` establishes an association or records a changed peer ID. The
/// existing key keeps its creation timestamp. `Removed` drops that key.
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
            let key = SessionKey::with_name(harness_id, cwd, session_name.as_deref());
            let now = chrono::Utc::now();
            if let Some(existing) = store.iter_mut().find(|record| stored_key(record) == key) {
                existing.acp_session_id = acp_session_id.clone();
                existing.last_used_at = now;
            } else {
                store.push(crate::acp::session::PersistedAcpSession {
                    harness_id: harness_id.clone(),
                    acp_session_id: acp_session_id.clone(),
                    cwd: cwd.clone(),
                    created_at: now,
                    last_used_at: now,
                    session_name: session_name.clone(),
                });
            }
        }
        AcpSessionEvent::Removed {
            harness_id,
            cwd,
            session_name,
        } => {
            let key = SessionKey::with_name(harness_id, cwd, session_name.as_deref());
            store.retain(|record| stored_key(record) != key);
        }
    }
}

fn stored_key(record: &crate::acp::session::PersistedAcpSession) -> SessionKey {
    SessionKey::with_name(
        &record.harness_id,
        &record.cwd,
        record.session_name.as_deref(),
    )
}

/// Owns the persistence worker until every accepted event has reached disk.
pub struct AcpPersistence {
    close: tokio::sync::oneshot::Sender<()>,
    worker: tokio::task::JoinHandle<Result<()>>,
    #[cfg(test)]
    sender: tokio::sync::mpsc::UnboundedSender<crate::acp::AcpSessionEvent>,
}

impl AcpPersistence {
    /// Stop accepting events, drain the queue, and observe the final write.
    pub async fn close_and_await(self) -> Result<()> {
        let _ = self.close.send(());
        self.worker
            .await
            .map_err(|error| AlephError::other(format!("ACP persistence worker failed: {error}")))?
    }
}

/// Install the writer before returning saved associations for caller-owned restore.
pub async fn wire_persistence(
    manager: &AcpAdapterManager,
) -> (
    AcpPersistence,
    Vec<crate::acp::session::PersistedAcpSession>,
) {
    let persisted = tokio::task::spawn_blocking(load_persisted_sessions)
        .await
        .unwrap_or_else(|error| {
            warn!(%error, "ACP session load task failed");
            Vec::new()
        });
    let mut sessions = persisted.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::acp::AcpSessionEvent>();
    let (close, mut closing) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let mut final_result = Ok(());
        loop {
            let event = tokio::select! {
                biased;
                _ = &mut closing => {
                    // Explicit receiver closure drains even while hooks retain senders.
                    rx.close();
                    break;
                }
                event = rx.recv() => event,
            };
            let Some(event) = event else {
                return final_result;
            };
            apply_event_to_store(&mut sessions, &event);
            final_result = persist_snapshot(sessions.clone()).await;
        }
        while let Some(event) = rx.recv().await {
            apply_event_to_store(&mut sessions, &event);
            final_result = persist_snapshot(sessions.clone()).await;
        }
        final_result
    });
    let hook_tx = tx.clone();
    manager
        .set_persistence_hook(Arc::new(move |event| {
            if hook_tx.send(event).is_err() {
                warn!("ACP persistence hook channel closed; session event dropped");
            }
        }))
        .await;
    (
        AcpPersistence {
            close,
            worker,
            #[cfg(test)]
            sender: tx,
        },
        persisted,
    )
}

async fn persist_snapshot(sessions: Vec<crate::acp::session::PersistedAcpSession>) -> Result<()> {
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let lock_path = acp_sessions_path().with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::utils::atomic_io::with_file_lock(&lock_path, |_guard| {
            write_sessions(&sessions).map_err(std::io::Error::other)
        })?;
        Ok(())
    })
    .await
    .map_err(|error| AlephError::other(format!("ACP persistence write task failed: {error}")))?;
    if let Err(error) = &result {
        warn!(%error, "ACP sessions on disk are behind the in-memory state");
    }
    result
}

// Exposed to sibling test modules in `acp::manager::tests` so they can pin
// the `ALEPH_HOME` override at the same path the persistence worker reads.
// Lives outside `mod wire_persistence_tests` because the cross-module
// consumer (`src/acp/manager/tests.rs`) needs `pub(super)` visibility,
// which a nested mod can't grant — and clippy's `items_after_test_module`
// lint rejects the original placement at end-of-file. Declared before the
// test module so the lint stays quiet without a per-function `#[allow]`.
#[cfg(test)]
pub(super) fn acp_sessions_path_for_test() -> std::path::PathBuf {
    acp_sessions_path()
}

#[cfg(test)]
mod wire_persistence_tests {
    //! Concurrency tests for the persistence worker.
    //!
    //! These tests construct an `AcpAdapterManager`, wire persistence against
    //! a temp directory (so we don't touch the real `~/.aleph/data/` file),
    //! fire a burst of `Created`/`Removed` events through retained
    //! senders, and verify the on-disk file reflects the cumulative state —
    //! i.e. that the stale-snapshot race is dead.

    use super::*;
    use crate::acp::manager::AcpAdapterManager;

    /// Point `acp_sessions_path()` at a scratch directory for the duration
    /// of `f` by overriding `ALEPH_HOME` — the variable
    /// `aleph_protocol::paths::data_dir` actually reads. The first cut of
    /// this helper set `ALEPH_DATA_DIR`, which nothing reads, so every run
    /// of these tests wrote 200 fake sessions into the developer's real
    /// `~/.aleph/data/acp_sessions.json`.
    ///
    /// Same discipline as every other `ALEPH_HOME` override in this crate:
    /// [`AlephHomeEnvGuard`] holds the process-wide guard so parallel tests
    /// cannot interleave their overrides, and restores the previous value
    /// on drop. The worker resolves the path on every write, so `f` must
    /// not return while writes are still queued — [`drain_and_load`] is how
    /// every test below makes sure of that before its last assertion.
    ///
    /// [`AlephHomeEnvGuard`]: crate::utils::paths::AlephHomeEnvGuard
    async fn with_temp_data_dir<F, Fut>(f: F)
    where
        F: FnOnce(std::path::PathBuf) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(tmp.path());
        f(tmp.path().to_path_buf()).await;
    }

    async fn drain_and_load(
        writer: AcpPersistence,
    ) -> Vec<crate::acp::session::PersistedAcpSession> {
        tokio::time::timeout(std::time::Duration::from_secs(30), writer.close_and_await())
            .await
            .expect("writer must drain despite retained hook sender")
            .expect("final write");
        load_persisted_sessions()
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

    #[tokio::test]
    async fn idle_writer_closes_with_live_manager_and_sender() {
        with_temp_data_dir(|_| async {
            let manager = AcpAdapterManager::new();
            let (writer, persisted) = wire_persistence(&manager).await;
            assert!(persisted.is_empty());
            let sender = writer.sender.clone();
            assert!(drain_and_load(writer).await.is_empty());
            assert!(sender.send(mk_event_created(1, "")).is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn wire_returns_saved_associations_without_changing_timestamps() {
        with_temp_data_dir(|_| async {
            let mut saved = Vec::new();
            apply_event_to_store(&mut saved, &mk_event_created(1, ""));
            let original = chrono::Utc::now() - chrono::Duration::days(1);
            saved[0].created_at = original;
            saved[0].last_used_at = original;
            write_sessions(&saved).unwrap();
            let manager = AcpAdapterManager::new();
            let (writer, persisted) = wire_persistence(&manager).await;
            assert_eq!(persisted.len(), 1);
            assert_eq!(persisted[0].created_at, original);
            assert_eq!(persisted[0].last_used_at, original);
            let on_disk = drain_and_load(writer).await;
            assert_eq!(on_disk[0].created_at, original);
            assert_eq!(on_disk[0].last_used_at, original);
        })
        .await;
    }

    #[tokio::test]
    async fn close_reports_final_write_failure() {
        with_temp_data_dir(|_| async move {
            let manager = AcpAdapterManager::new();
            let (writer, _) = wire_persistence(&manager).await;
            // A file at the data directory makes every real write fail.
            let data = acp_sessions_path().parent().unwrap().to_path_buf();
            std::fs::create_dir_all(data.parent().unwrap()).unwrap();
            std::fs::write(&data, b"blocked").unwrap();
            writer.sender.send(mk_event_created(1, "")).unwrap();
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(10), writer.close_and_await())
                    .await
                    .expect("failed writes must still join");
            assert!(result.is_err(), "final I/O failure must be observable");
        })
        .await;
    }

    #[test]
    fn existing_created_timestamp_is_preserved() {
        let mut store = Vec::new();
        apply_event_to_store(&mut store, &mk_event_created(1, ""));
        let original = chrono::Utc::now() - chrono::Duration::days(1);
        store[0].created_at = original;
        store[0].last_used_at = original;
        apply_event_to_store(&mut store, &mk_event_created(1, "-updated"));
        assert_eq!(store[0].created_at, original);
        assert!(store[0].last_used_at > original);
        assert_eq!(store[0].acp_session_id, "s-1-updated");
    }

    /// Fire a burst of Created events from N tasks concurrently. The
    /// final file must contain every (harness_id, cwd) tuple exactly once,
    /// proving no event was lost to the stale-snapshot race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn burst_of_creates_no_event_lost() {
        with_temp_data_dir(|_| async {
            let manager = AcpAdapterManager::new();
            let (writer, _) = wire_persistence(&manager).await;
            let tx = writer.sender.clone();

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
            let on_disk = drain_and_load(writer).await;
            assert!(tx.send(mk_event_created(N, "")).is_err());
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
        })
        .await;
    }

    /// Interleave Created + Removed for the same key. The final on-disk
    /// state must show empty — never a Created that was supposed to be
    /// cancelled by a later Removed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn interleaved_create_remove_no_cancelled_create() {
        with_temp_data_dir(|_| async {
            let manager = AcpAdapterManager::new();
            let (writer, _) = wire_persistence(&manager).await;
            let tx = writer.sender.clone();

            const N: usize = 100;
            for i in 0..N {
                let _ = tx.send(mk_event_created(i, ""));
                let _ = tx.send(mk_event_removed(i));
            }
            let on_disk = drain_and_load(writer).await;
            assert!(
                on_disk.is_empty(),
                "after {N} Created+Removed pairs the store must be empty, \
                 found {} entries — Removed lost to stale-snapshot race?",
                on_disk.len()
            );
        })
        .await;
    }

    /// Sanity: the worker preserves `created_at` across repeated Created
    /// events for the same triple (idempotent re-emit after every prompt).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_created_preserves_created_at() {
        with_temp_data_dir(|_| async {
            let manager = AcpAdapterManager::new();
            let (writer, _) = wire_persistence(&manager).await;
            let tx = writer.sender.clone();

            let before = chrono::Utc::now();
            for _ in 0..5 {
                let _ = tx.send(mk_event_created(7, ""));
            }
            let on_disk = drain_and_load(writer).await;
            let after = chrono::Utc::now();

            assert_eq!(on_disk.len(), 1, "exactly one entry expected");
            let entry = &on_disk[0];
            assert!(
                entry.created_at <= after && entry.created_at >= before,
                "created_at {} must lie within [{}, {}]",
                entry.created_at,
                before,
                after,
            );
        })
        .await;
    }
}
