//! Note-vault watcher — keep the index honest about markdown edited *outside*
//! Aleph.
//!
//! ## Why this exists
//!
//! The note tree is the source of truth: `notes_index`, FTS and the vector
//! tables are all derived from `note/{agent_id}/{category}/*.md`, and every
//! agent directory gets an `.obsidian/` vault config written into it
//! ([`crate::memory::notes::orientation::ensure_obsidian_config`]) precisely so
//! the user can open their memory in Obsidian, VSCode or Vim and edit it.
//!
//! Nothing honoured that. An edit made outside Aleph changed the truth and
//! nothing downstream noticed: search returned the old body, the graph kept the
//! old edges, and the note's vector went on describing text that no longer
//! existed — until the next process restart, which reconciled *one* corpus. A
//! deletion was worse: the index row survived, so recall kept surfacing a note
//! whose file was gone.
//!
//! ## What it does
//!
//! One debounced recursive watch over the whole note root. For each `.md` path
//! that settles, the current state **of the filesystem** decides the action —
//! not the event kind, which is unreliable across platforms and coalescing:
//!
//! - the file exists → [`NoteIndexer::index_file`], which no-ops on an unchanged
//!   content hash (so Aleph's own writes, which already indexed themselves, cost
//!   one hash comparison) and otherwise re-indexes, re-embeds and re-resolves
//!   inbound links;
//! - the file is gone (`NotFound`, specifically) → the index row is removed with
//!   the usual tombstone semantics for inbound links.
//!
//! A read error that is *not* `NotFound` (a permissions problem, an unmounted
//! network volume) is logged and skipped: "I could not look" is not evidence of
//! "it does not exist", and the action on that branch is a delete.
//!
//! An oversized batch — a vault sync, a `git checkout`, a bulk import — is
//! handled as a whole-corpus reconcile instead of thousands of individual
//! round-trips. [`NoteIndexer::reconcile_corpus`] skips unchanged files by hash
//! too, so the bulk path is the cheap one at that size.
//!
//! ## Deliberately not a knob
//!
//! There is no config flag to turn this off. It does not choose a behaviour on
//! the user's behalf; it makes the index tell the truth about the files the user
//! already owns. Failure is graceful — if the watch cannot be established the
//! caller logs it and the process behaves exactly as it did before this module
//! existed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{NoteIndexer, CATEGORY_DIRS};
use crate::sync_primitives::Arc;

/// Settle window before a batch of filesystem events is acted on. Editors write
/// through temp files and swap; Obsidian in particular touches a note several
/// times per save. Long enough to collapse those into one reconcile, short
/// enough that an edit is searchable before the user switches back.
const DEBOUNCE_MS: u64 = 750;

/// Above this many changed files in one settled batch, reconcile the affected
/// corpora wholesale rather than file by file.
const MAX_PATHS_PER_BATCH: usize = 128;

/// Live watch over the note vault. Dropping it stops the watch.
///
/// Must be held for as long as the vault should stay reconciled — the debouncer
/// owns the platform watch handle and the background thread that feeds it.
pub struct NoteVaultWatcher {
    _debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
}

/// Where a changed file sits in the note layout.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteKey {
    agent_id: String,
    category: String,
    title: String,
}

impl NoteKey {
    /// The `"{category}/{title}"` key every note table is indexed by.
    fn path(&self) -> String {
        format!("{}/{}", self.category, self.title)
    }
}

/// Classify a changed path as an indexable note, or reject it.
///
/// Accepts exactly `{root}/{agent_id}/{category}/{title}.md` — the one shape
/// `full_rebuild` scans. Everything else in the tree is deliberately out:
/// per-agent scaffold at depth 2 (`index.md`, `SCHEMA.md`, `LOG.md`), the
/// `archive/` graveyard and any other non-[`CATEGORY_DIRS`] directory (both
/// invisible to the active index by design), dot-directories (`.obsidian/`) and
/// atomic-write staging files (`.aleph_atomic_*.tmp`, filtered earlier by
/// extension).
fn classify(root: &Path, path: &Path) -> Option<NoteKey> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return None;
    }
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    let [agent_id, category, _file] = parts.as_slice() else {
        return None;
    };
    if agent_id.is_empty() || agent_id.starts_with('.') {
        return None;
    }
    if !CATEGORY_DIRS.contains(category) {
        return None;
    }
    let title = path.file_stem().and_then(|s| s.to_str())?;
    if title.is_empty() {
        return None;
    }
    Some(NoteKey {
        agent_id: (*agent_id).to_string(),
        category: (*category).to_string(),
        title: title.to_string(),
    })
}

/// Start watching the indexer's note root for external markdown edits.
///
/// Returns the live watch; drop it to stop. Errors when there is no note root on
/// disk yet or the platform watch cannot be established — this is a sensor, so
/// it never creates the directory it measures.
///
/// Must be called from inside a Tokio runtime: the debouncer's callback runs on
/// its own thread and hands work to a task on the current runtime.
pub fn spawn_note_vault_watcher<S>(
    indexer: Arc<NoteIndexer<S>>,
) -> Result<NoteVaultWatcher, AlephError>
where
    S: NoteStore + Send + Sync + 'static,
{
    let root = indexer.memory_dir().to_path_buf();
    if !root.is_dir() {
        return Err(AlephError::config(format!(
            "note vault watcher: {} is not a directory",
            root.display()
        )));
    }
    // Watch — and strip — the CANONICAL root. Filesystem notifications report
    // canonical paths (macOS resolves `/var` → `/private/var`, and any home or
    // volume symlink in the data-dir path resolves the same way), so a watcher
    // that kept the un-resolved root would `strip_prefix` every incoming path to
    // `None` and quietly classify the entire vault as "not a note". The whole
    // subsystem would then be *running* and doing nothing, which is the failure
    // mode with no symptom at all.
    let root = root.canonicalize().unwrap_or(root);
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| AlephError::config(format!("note vault watcher needs a runtime: {e}")))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();

    // Unbounded is safe here because the *producer* is rate-limited by the
    // debounce window: at most one message per DEBOUNCE_MS, each carrying the
    // paths that settled in it. The size a batch can reach is bounded instead —
    // see MAX_PATHS_PER_BATCH.
    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let mut paths: Vec<PathBuf> = events
                    .iter()
                    .flat_map(|e| e.paths.iter().cloned())
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
                    .collect();
                paths.sort();
                paths.dedup();
                if paths.is_empty() {
                    return;
                }
                // A closed receiver means the watcher outlived its task; the
                // watch is about to be dropped with it.
                let _ = tx.send(paths);
            }
            Err(errors) => {
                for error in &errors {
                    tracing::warn!(?error, "note vault watcher error");
                }
            }
        },
    )
    .map_err(|e| AlephError::config(format!("note vault watcher: {e}")))?;

    debouncer
        .watcher()
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| {
            AlephError::config(format!("note vault watcher: watch {}: {e}", root.display()))
        })?;

    let watch_root = root.clone();
    handle.spawn(async move {
        while let Some(paths) = rx.recv().await {
            reconcile_batch(&indexer, &watch_root, paths).await;
        }
    });

    tracing::info!(root = %root.display(), "Watching note vault for external edits");
    Ok(NoteVaultWatcher {
        _debouncer: debouncer,
    })
}

/// Reconcile one settled batch of changed paths.
async fn reconcile_batch<S>(indexer: &NoteIndexer<S>, root: &Path, paths: Vec<PathBuf>)
where
    S: NoteStore + Send + Sync + 'static,
{
    let keys: Vec<(PathBuf, NoteKey)> = paths
        .into_iter()
        .filter_map(|p| classify(root, &p).map(|k| (p, k)))
        .collect();
    if keys.is_empty() {
        return;
    }

    if keys.len() > MAX_PATHS_PER_BATCH {
        let corpora: BTreeSet<&str> = keys.iter().map(|(_, k)| k.agent_id.as_str()).collect();
        tracing::info!(
            files = keys.len(),
            corpora = corpora.len(),
            "note vault: bulk change, reconciling affected corpora wholesale"
        );
        for corpus in corpora {
            // `reconcile_corpus`, not `full_rebuild`: a watcher reacts to what
            // the user did to their files, and provisioning 21 category
            // directories is not part of that.
            if let Err(e) = indexer.reconcile_corpus(corpus).await {
                tracing::warn!(corpus, error = %e, "note vault: bulk reconcile failed");
            }
        }
        return;
    }

    for (path, key) in keys {
        match tokio::fs::metadata(&path).await {
            Ok(_) => match indexer
                .index_file(&key.agent_id, &key.category, &path)
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        agent = %key.agent_id,
                        note = %key.path(),
                        "note vault: re-indexed externally edited note"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(note = %key.path(), error = %e, "note vault: re-index failed");
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match indexer
                    .store()
                    .remove_note_index(&key.path(), &key.agent_id)
                    .await
                {
                    Ok(()) => {
                        tracing::info!(
                            agent = %key.agent_id,
                            note = %key.path(),
                            "note vault: dropped index row for a note deleted on disk"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(note = %key.path(), error = %e, "note vault: index drop failed");
                    }
                }
            }
            // "I could not look" is not evidence of "it does not exist", and the
            // other branch deletes. Skip; the next boot reconcile settles it.
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "note vault: stat failed, skipping");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/vault/note")
    }

    #[test]
    fn classifies_a_note_under_a_known_category() {
        let k = classify(&root(), Path::new("/vault/note/main/reference/rust.md")).unwrap();
        assert_eq!(
            k,
            NoteKey {
                agent_id: "main".into(),
                category: "reference".into(),
                title: "rust".into(),
            }
        );
        assert_eq!(k.path(), "reference/rust");
    }

    #[test]
    fn classifies_a_scoped_corpus_the_same_way_as_a_base_one() {
        let k = classify(
            &root(),
            Path::new("/vault/note/main__proj-abc123/plan/ship.md"),
        )
        .unwrap();
        assert_eq!(k.agent_id, "main__proj-abc123");
        assert_eq!(k.path(), "plan/ship");
    }

    /// `archive/` is where `NoteDecay` parks cold notes and is deliberately
    /// absent from `CATEGORY_DIRS`; re-indexing from there would resurrect
    /// every archived note the moment its file was touched.
    #[test]
    fn rejects_archive_and_other_unknown_categories() {
        assert!(classify(&root(), Path::new("/vault/note/main/archive/old.md")).is_none());
        assert!(classify(&root(), Path::new("/vault/note/main/scratch/x.md")).is_none());
    }

    /// Agent-root scaffold (`index.md`, `SCHEMA.md`, `LOG.md`, `USER.md`) is
    /// generated, not indexed — and it is written on every dream cycle, so
    /// accepting it would mean a reconcile per cycle for nothing.
    #[test]
    fn rejects_agent_root_scaffold_and_deeper_paths() {
        assert!(classify(&root(), Path::new("/vault/note/main/index.md")).is_none());
        assert!(classify(&root(), Path::new("/vault/note/main/plan/sub/deep.md")).is_none());
        assert!(classify(&root(), Path::new("/vault/note/loose.md")).is_none());
    }

    #[test]
    fn rejects_non_markdown_and_dot_directories() {
        assert!(classify(&root(), Path::new("/vault/note/main/plan/a.txt")).is_none());
        assert!(classify(
            &root(),
            Path::new("/vault/note/main/plan/.aleph_atomic_1.tmp")
        )
        .is_none());
        assert!(classify(&root(), Path::new("/vault/note/.trash/plan/a.md")).is_none());
    }

    #[test]
    fn rejects_paths_outside_the_watched_root() {
        assert!(classify(&root(), Path::new("/elsewhere/main/plan/a.md")).is_none());
    }

    /// End-to-end: a real watch over a real directory, a real editor-style
    /// write, a real index row.
    ///
    /// The `classify` tests above only prove the path grammar. What actually
    /// broke for years is the *wire* — nobody was watching at all — so this
    /// asserts the effect at the far end (a queryable note appears, then
    /// disappears when the file is deleted), not that a function was called.
    /// Polls with a ceiling rather than sleeping a fixed time: FSEvents and
    /// inotify latencies differ by an order of magnitude.
    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_indexes_a_file_created_outside_aleph_and_drops_it_on_delete() {
        use crate::memory::notes::NoteIndexer;
        use crate::memory::store::SqliteMemoryBackend;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("note");
        let cat = root.join("main").join("reference");
        tokio::fs::create_dir_all(&cat).await.unwrap();

        let db: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        // rust-doctor-disable-next-line excessive-clone
        let indexer = Arc::new(NoteIndexer::new(root.clone(), db.clone()));
        let _watch = spawn_note_vault_watcher(indexer).expect("watcher must start");

        // Someone writes a note in Obsidian.
        let file = cat.join("external.md");
        tokio::fs::write(
            &file,
            "---\ncategory: reference\ntags: []\n---\n\n- written outside Aleph\n",
        )
        .await
        .unwrap();
        assert!(
            wait_until(|| async { !db.list_notes("main").await.unwrap_or_default().is_empty() })
                .await,
            "an externally created note must reach the index"
        );

        // …and then deletes it.
        tokio::fs::remove_file(&file).await.unwrap();
        assert!(
            wait_until(|| async { db.list_notes("main").await.unwrap_or_default().is_empty() })
                .await,
            "deleting the file must drop the index row, not leave a recallable ghost"
        );
    }

    /// Poll `cond` until true or the ceiling elapses. Returns whether it held.
    #[cfg(test)]
    async fn wait_until<F, Fut>(mut cond: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        const CEILING: Duration = Duration::from_secs(20);
        let deadline = std::time::Instant::now() + CEILING;
        while std::time::Instant::now() < deadline {
            if cond().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }
}
