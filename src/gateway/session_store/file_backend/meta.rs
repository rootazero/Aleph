//! The `metadata.json` read-modify-write chokepoint.
//!
//! # Why this module exists
//!
//! Every field of a session — its dials, its token counters, its title, its
//! lifecycle state — lives in one JSON document that is rewritten whole. The
//! backend never updates a column; it reads the document, changes one field,
//! and writes it back. Fifteen call sites do that, and they overlap routinely:
//! a turn stamps `last_active_at` while the projector records usage while the
//! user flips a dial.
//!
//! Two earlier rounds fixed two different halves of the damage that causes:
//!
//! 1. The write was `fs::write` (`create + truncate + write_all`), so a second
//!    writer could truncate between the first writer's open and its write and
//!    leave a hybrid document on disk — which parses as nothing, and makes the
//!    conversation vanish from every surface at once. Fixed by writing
//!    atomically (temp file in the same directory, fsync, rename).
//! 2. Atomicity alone still loses updates: both writers read the same document,
//!    each changes its own field, and whoever renames last silently reverts the
//!    other's field. The survivor is a *complete* document — it is simply a
//!    document that is missing an update somebody was told had been saved.
//!
//! This module closes (2). A write can only be produced by a [`MetaGuard`], and
//! a guard can only be produced by [`MetaLocks::lock`], which acquires the
//! per-session lock **and then** reads the document. So "read, change, write"
//! is one critical section by construction, and there is no way to spell the
//! bug: the private `write` function below is unreachable from the parent
//! module. That is the point of the module boundary — a source-level guard
//! could only recognise the shapes it was taught, and this one cannot be
//! bypassed by a shape nobody thought of.
//!
//! The twin backend already answers this correctly, which is how the shape was
//! confirmed rather than guessed: `SessionManager::patch_session` (SQLite)
//! holds the connection mutex across its `SELECT metadata` and its `UPDATE`,
//! so its read-modify-write of the same `custom` blob is one critical section
//! by construction. The file backend was the only one of the two that had no
//! such boundary.
//!
//! # Scope of the lock
//!
//! In-process, per session key. That is the right scope because the process is
//! a singleton: `~/.aleph/data/aleph.lock` is an OS-level `flock`, and CLI
//! write subcommands go through IPC rather than opening the store themselves
//! (see `docs/reference/PROCESS_MANAGEMENT.md`). A second process writing this
//! directory is already a diagnosed fault (`doctor`'s
//! `core/duplicate-instance`), not a case to be locked against here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::SessionMetadata;

/// Prune dead slots once the table reaches this size.
///
/// A slot whose `Arc` is gone has no live critical section, so dropping it
/// cannot split a lock in two. The bound only exists so a long-lived server
/// that touches many short-lived sessions does not accumulate one `Weak` per
/// key forever.
const PRUNE_AT: usize = 128;

/// Per-session-key write locks for `metadata.json`.
#[derive(Debug, Default)]
pub(crate) struct MetaLocks {
    slots: std::sync::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl MetaLocks {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Resolve the lock for `key`, creating it if no live holder exists.
    ///
    /// Upgrade-or-insert happens under the table's own mutex, so two tasks
    /// racing on the same key always end up with the same `Arc` — the check
    /// and the insert cannot interleave.
    fn slot(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(live) = slots.get(key).and_then(Weak::upgrade) {
            return live;
        }
        if slots.len() >= PRUNE_AT {
            slots.retain(|_, w| w.strong_count() > 0);
        }
        let fresh = Arc::new(tokio::sync::Mutex::new(()));
        slots.insert(key.to_string(), Arc::downgrade(&fresh));
        fresh
    }

    /// Take the session's write lock and read its metadata under it.
    ///
    /// The returned guard holds the lock until it is committed or dropped.
    /// Dropping without [`MetaGuard::commit`] writes nothing, which is what an
    /// early return wants (`close_session` on an already-stopped session,
    /// `backfill_attribution` on an already-stamped one).
    pub(crate) async fn lock(
        &self,
        key: &str,
        path: PathBuf,
    ) -> Result<MetaGuard, SessionStoreError> {
        let permit = self.slot(key).lock_owned().await;
        let meta = read(&path).await?;
        Ok(MetaGuard {
            path,
            meta,
            _permit: permit,
        })
    }

    /// How many slots the table is holding. Test-only observability for the
    /// pruning bound.
    #[cfg(test)]
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Exclusive access to one session's metadata document.
///
/// Obtained from [`MetaLocks::lock`], which reads the document **after** taking
/// the lock — so what the guard holds is what is on disk right now, and it
/// cannot change under it.
#[derive(Debug)]
pub(crate) struct MetaGuard {
    path: PathBuf,
    meta: Option<SessionMetadata>,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

impl MetaGuard {
    /// Mutable access to the existing document. `None` means there is nothing
    /// to update — use [`Self::insert`] to create one.
    pub(crate) const fn existing_mut(&mut self) -> Option<&mut SessionMetadata> {
        self.meta.as_mut()
    }

    /// Install a document, replacing whatever the guard read.
    ///
    /// Used by the two creation paths (a brand-new session, and the new key a
    /// checkpoint branch materializes). Creation is a read-modify-write like
    /// any other — "does this session exist? no, create it" — so it is held
    /// under the same lock rather than written blind.
    pub(crate) fn insert(&mut self, meta: SessionMetadata) -> &mut SessionMetadata {
        self.meta.insert(meta)
    }

    /// Write the document back and release the lock.
    ///
    /// Returns what was written, so callers can emit their `sessions.changed`
    /// event from the same value rather than reading the file again.
    ///
    /// Committing a guard that holds no document is an error rather than a
    /// silent no-op: every caller reaches here having either mutated
    /// `existing_mut()` or `insert()`ed one, so an absent document means the
    /// caller's own control flow skipped both. The paths that legitimately
    /// write nothing — an already-stopped session, an already-stamped one —
    /// drop the guard instead, and say so where they do it.
    pub(crate) async fn commit(self) -> Result<SessionMetadata, SessionStoreError> {
        let Some(meta) = self.meta else {
            return Err(SessionStoreError::DatabaseError(
                "commit with no metadata to write".to_string(),
            ));
        };
        write(&self.path, &meta).await?;
        Ok(meta)
    }
}

/// Read and parse a metadata document. A missing file is `Ok(None)`; an
/// unparseable one is an error, never an absence — see the note on
/// `FileSessionStore::read_metadata`.
pub(crate) async fn read(path: &Path) -> Result<Option<SessionMetadata>, SessionStoreError> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SessionStoreError::DatabaseError(format!(
                "Failed to read metadata: {e}"
            )))
        }
    };
    let meta: SessionMetadata = serde_json::from_str(&contents)
        .map_err(|e| SessionStoreError::DatabaseError(format!("Failed to parse metadata: {e}")))?;
    Ok(Some(meta))
}

/// Persist a metadata document atomically.
///
/// Private to this module on purpose: reaching it requires a [`MetaGuard`],
/// which requires the lock. See the module doc.
///
/// # Why atomic and not `fs::write`
///
/// `fs::write` is `create + truncate + write_all`, and both halves are
/// observable. Writer B can truncate *after* writer A has opened but *before*
/// A writes, so A's bytes land and then B's shorter document overwrites only
/// A's prefix. What survives is B's document followed by the tail of A's — 509
/// valid bytes plus 58 bytes of an older one, in the case that was actually
/// caught on a live server.
///
/// The cost of that is out of all proportion to how it reads. Nothing crashes
/// and nothing is logged: `list_sessions` skips a `metadata.json` it cannot
/// parse and `read_metadata` fails, so the conversation disappears from every
/// surface at once — absent from `sessions.list`, `chat.history` answers
/// "session not found", `sessions.patch` refuses it — while `transcript.jsonl`
/// sits intact beside the broken file. It survives a restart, because it is
/// on-disk damage rather than lost in-memory state, so the one remedy a user
/// would try does not work.
///
/// The per-session lock above now means two writers no longer overlap at all.
/// This stays atomic anyway: it is what bounds the damage from a crash or a
/// full disk *during* the write, which no lock can prevent.
async fn write(path: &Path, meta: &SessionMetadata) -> Result<(), SessionStoreError> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create session dir: {e}"))
        })?;
    }
    let contents = serde_json::to_string_pretty(meta).map_err(|e| {
        SessionStoreError::DatabaseError(format!("Failed to serialize metadata: {e}"))
    })?;
    crate::utils::atomic_write::atomic_write_file(path, &contents)
        .await
        .map_err(|e| SessionStoreError::DatabaseError(format!("Failed to write metadata: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with_title(title: &str) -> SessionMetadata {
        SessionMetadata {
            key: "agent:main:main".to_string(),
            derived_title: Some(title.to_string()),
            ..Default::default()
        }
    }

    /// The same key resolves to the same lock while a holder is alive — the
    /// property the whole module rests on. If `slot` handed out two different
    /// mutexes for one key, every guarantee above would be decoration.
    #[tokio::test]
    async fn one_key_is_one_lock_while_a_holder_is_alive() {
        let locks = MetaLocks::new();
        let a = locks.slot("agent:main:main");
        let b = locks.slot("agent:main:main");
        assert!(Arc::ptr_eq(&a, &b));
        let c = locks.slot("agent:main:other");
        assert!(!Arc::ptr_eq(&a, &c));
    }

    /// Dropping a guard without committing writes nothing. Early returns
    /// (`close_session` on a stopped session) rely on this.
    #[tokio::test]
    async fn a_dropped_guard_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s").join("metadata.json");
        let locks = MetaLocks::new();

        let mut guard = locks.lock("k", path.clone()).await.unwrap();
        guard.insert(meta_with_title("never"));
        drop(guard);

        assert!(!path.exists());
    }

    /// Two concurrent read-modify-writes against the same key both survive.
    /// This is the lost-update regression: without the lock the second writer
    /// reads the pre-first document and its commit reverts the first field.
    #[tokio::test]
    async fn concurrent_updates_do_not_lose_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s").join("metadata.json");
        let locks = Arc::new(MetaLocks::new());

        // Seed.
        let mut seed = locks.lock("k", path.clone()).await.unwrap();
        seed.insert(SessionMetadata {
            key: "k".to_string(),
            ..Default::default()
        });
        seed.commit().await.unwrap();

        // Writer A bumps tokens 64 times; writer B sets a title and bumps the
        // message count 64 times. Interleaved, unlocked, one of the two fields
        // ends up reverted.
        let a = {
            let locks = Arc::clone(&locks);
            let path = path.clone();
            tokio::spawn(async move {
                for _ in 0..64 {
                    let mut g = locks.lock("k", path.clone()).await.unwrap();
                    g.existing_mut().unwrap().total_tokens += 1;
                    tokio::task::yield_now().await;
                    g.commit().await.unwrap();
                }
            })
        };
        let b = {
            let locks = Arc::clone(&locks);
            let path = path.clone();
            tokio::spawn(async move {
                for _ in 0..64 {
                    let mut g = locks.lock("k", path.clone()).await.unwrap();
                    let m = g.existing_mut().unwrap();
                    m.message_count += 1;
                    m.derived_title = Some("kept".to_string());
                    tokio::task::yield_now().await;
                    g.commit().await.unwrap();
                }
            })
        };
        a.await.unwrap();
        b.await.unwrap();

        let final_meta = read(&path).await.unwrap().unwrap();
        assert_eq!(final_meta.total_tokens, 64, "writer A lost updates");
        assert_eq!(final_meta.message_count, 64, "writer B lost updates");
        assert_eq!(final_meta.derived_title.as_deref(), Some("kept"));
    }

    /// The slot table does not grow without bound across many short-lived
    /// sessions.
    #[tokio::test]
    async fn dead_slots_are_reclaimed() {
        let locks = MetaLocks::new();
        for i in 0..(PRUNE_AT * 3) {
            drop(locks.slot(&format!("agent:main:s{i}")));
        }
        assert!(
            locks.slot_count() <= PRUNE_AT,
            "slot table kept {} entries",
            locks.slot_count()
        );
    }
}
