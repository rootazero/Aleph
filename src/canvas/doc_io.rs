//! Read-modify-write for `doc.json` is one critical section BY CONSTRUCTION:
//! `write` is private; only a [`DocGuard`] (from [`DocLocks::lock`], which
//! takes the per-canvas mutex THEN reads) can commit. Mirrors
//! `gateway/session_store/file_backend/meta.rs` — read that module doc first
//! for the full damage report this shape exists to prevent (torn writes,
//! lost updates, a conversation vanishing from every surface at once).
//!
//! One deliberate difference from the twin: [`DocGuard::commit`] takes
//! `&mut self` and KEEPS the lock. The store publishes its `canvas.updated`
//! event from the committed document while still inside the critical section
//! (roster precedent: mutation + snapshot + publish in one lock scope), so
//! event order can never diverge from commit order. A `commit(self)` that
//! consumed the guard would drop the per-canvas mutex before the caller's
//! publish ran, and two racing applies could then publish in reverse
//! revision order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use aleph_protocol::canvas::CanvasDoc;

use crate::sync_primitives::Mutex;

use super::store::CanvasError;

/// Prune dead slots once the table reaches this size.
///
/// A slot whose `Arc` is gone has no live critical section, so dropping it
/// cannot split a lock in two. The bound only exists so a long-lived server
/// that touches many short-lived canvases does not accumulate one `Weak` per
/// id forever. Same constant, same reasoning as `MetaLocks`.
const PRUNE_AT: usize = 128;

/// Per-canvas-id write locks for `doc.json`.
#[derive(Debug, Default)]
pub(super) struct DocLocks {
    slots: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl DocLocks {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Resolve the lock for `id`, creating it if no live holder exists.
    ///
    /// Upgrade-or-insert happens under the table's own mutex, so two tasks
    /// racing on the same id always end up with the same `Arc` — the check
    /// and the insert cannot interleave.
    fn slot(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| { tracing::error!(
                reason = %e,
                "canvas lock table poisoned: a previous holder panicked mid-insert; recovering"
            ); e.into_inner() });
        if let Some(live) = slots.get(id).and_then(Weak::upgrade) {
            return live;
        }
        if slots.len() >= PRUNE_AT {
            slots.retain(|_, w| w.strong_count() > 0);
        }
        let fresh = Arc::new(tokio::sync::Mutex::new(()));
        slots.insert(id.to_string(), Arc::downgrade(&fresh));
        fresh
    }

    /// Take the canvas's write lock and read its document under it.
    ///
    /// The returned guard holds the lock until it is dropped. Dropping
    /// without [`DocGuard::commit`] writes nothing, which is what a rejected
    /// batch wants: `apply` mutates the guarded document in place and bails
    /// on validation failure, and the half-applied in-memory copy dies with
    /// the guard instead of landing on disk.
    pub(super) async fn lock(&self, id: &str, path: PathBuf) -> Result<DocGuard, CanvasError> {
        let permit = self.slot(id).lock_owned().await;
        let doc = read(&path).await?;
        Ok(DocGuard {
            path,
            doc,
            _permit: permit,
        })
    }

    /// How many slots the table is holding. Test-only observability for the
    /// pruning bound.
    #[cfg(test)]
    pub(super) fn slot_count(&self) -> usize {
        self.slots.lock().unwrap_or_else(|e| { tracing::error!(
                reason = %e,
                "canvas lock table poisoned: a previous holder panicked mid-insert; recovering"
            ); e.into_inner() }).len()
    }
}

/// Exclusive access to one canvas document.
///
/// Obtained from [`DocLocks::lock`], which reads the document **after**
/// taking the lock — so what the guard holds is what is on disk right now,
/// and it cannot change under it.
#[derive(Debug)]
pub(super) struct DocGuard {
    path: PathBuf,
    doc: Option<CanvasDoc>,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

impl DocGuard {
    /// Mutable access to the existing document. `None` means there is no
    /// such canvas — use [`Self::insert`] to create one.
    pub(super) const fn existing_mut(&mut self) -> Option<&mut CanvasDoc> {
        self.doc.as_mut()
    }

    /// Install a document, replacing whatever the guard read.
    ///
    /// Creation is a read-modify-write like any other — "does this canvas
    /// exist? no, create it" — so it is held under the same lock rather than
    /// written blind (`MetaGuard` precedent, and §5.23b: the creation path
    /// belongs inside the critical section too).
    pub(super) fn insert(&mut self, doc: CanvasDoc) -> &mut CanvasDoc {
        self.doc.insert(doc)
    }

    /// Write the document back, KEEPING the lock (see the module doc for why
    /// this is `&mut self` and not `self`).
    ///
    /// Returns a borrow of what was written, so the caller can publish its
    /// `canvas.updated` event from the same value — still inside the
    /// critical section — rather than reading the file again.
    ///
    /// Committing a guard that holds no document is an error rather than a
    /// silent no-op: every caller reaches here having either mutated
    /// `existing_mut()` or `insert()`ed one, so an absent document means the
    /// caller's own control flow skipped both.
    pub(super) async fn commit(&mut self) -> Result<&CanvasDoc, CanvasError> {
        let doc = self.doc.as_ref().ok_or_else(|| {
            CanvasError::Internal("commit with no canvas document to write".to_string())
        })?;
        write(&self.path, doc).await?;
        Ok(self.doc.as_ref().expect("checked above"))
    }
}

/// Read and parse a canvas document. A missing file is `Ok(None)`; an
/// unparseable one is an error, never an absence — "failed to parse" and
/// "does not exist" are two different answers (§0), and folding the former
/// into the latter is how a corrupt document becomes invisible on every
/// surface at once.
pub(super) async fn read(path: &Path) -> Result<Option<CanvasDoc>, CanvasError> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CanvasError::Internal(format!(
                "failed to read canvas doc: {e}"
            )))
        }
    };
    let doc: CanvasDoc = serde_json::from_str(&contents)
        .map_err(|e| CanvasError::Internal(format!("failed to parse canvas doc: {e}")))?;
    Ok(Some(doc))
}

/// Persist a canvas document atomically.
///
/// Private to this module on purpose: reaching it requires a [`DocGuard`],
/// which requires the lock. Atomicity stays even though the lock already
/// serializes writers — it is what bounds the damage from a crash or a full
/// disk *during* the write, which no lock can prevent.
async fn write(path: &Path, doc: &CanvasDoc) -> Result<(), CanvasError> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| CanvasError::Internal(format!("failed to create canvas dir: {e}")))?;
    }
    let contents = serde_json::to_string_pretty(doc)
        .map_err(|e| CanvasError::Internal(format!("failed to serialize canvas doc: {e}")))?;
    crate::utils::atomic_write::atomic_write_file(path, &contents)
        .await
        .map_err(|e| CanvasError::Internal(format!("failed to write canvas doc: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same id resolves to the same lock while a holder is alive — the
    /// property the whole module rests on.
    #[tokio::test]
    async fn one_id_is_one_lock_while_a_holder_is_alive() {
        let locks = DocLocks::new();
        let a = locks.slot("cv-1");
        let b = locks.slot("cv-1");
        assert!(Arc::ptr_eq(&a, &b));
        let c = locks.slot("cv-2");
        assert!(!Arc::ptr_eq(&a, &c));
    }

    /// The slot table does not grow without bound across many short-lived
    /// canvases.
    #[tokio::test]
    async fn dead_slots_are_reclaimed() {
        let locks = DocLocks::new();
        for i in 0..(PRUNE_AT * 3) {
            drop(locks.slot(&format!("cv-{i}")));
        }
        assert!(
            locks.slot_count() <= PRUNE_AT,
            "slot table kept {} entries",
            locks.slot_count()
        );
    }

    /// Dropping a guard without committing writes nothing — the property the
    /// rejected-batch path (`apply_ops` failure) relies on.
    #[tokio::test]
    async fn a_dropped_guard_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cv-x").join("doc.json");
        let locks = DocLocks::new();

        let mut guard = locks.lock("cv-x", path.clone()).await.unwrap();
        guard.insert(CanvasDoc {
            id: "cv-x".to_string(),
            title: "never".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes: Vec::new(),
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        });
        drop(guard);

        assert!(!path.exists());
    }
}
