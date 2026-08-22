//! Shared test fixtures for the memory events layer.
//!
//! Construction helpers that other modules need but no longer have to
//! duplicate. Each helper sets up its own scratch directory / temp
//! file so the returned state is fully owned by the test \u2014 binding
//! the returned guards in the test frame is what keeps the underlying
//! `SQLite` / filesystem alive for the duration of the test.
//!
//! Currently exported:
//!
//! * [`make_handler_with_indexer`] \u2014 a `MemoryCommandHandler` wired
//!   to a real on-disk `NoteIndexer` + a `StateDatabase` so callers
//!   can exercise the dual-write path (event log + filesystem
//!   projection) and the reconciler's divergence detection without
//!   standing up a full aleph-server.
//!
//! The fixture modules at `memory::notes::indexer::tests` and
//! `memory::events::handler::tests` still own their own helpers;
//! migration is incremental as more test files reach for the
//! shared path.
//!
//! The module is gated on `test-helpers` (and `test`) so the
//! `scratch_root` helper, which is `#[cfg(any(test, feature =
//! "test-helpers"))]`, stays accessible to consumers that opt into
//! the same gate via a feature flag.

#[cfg(any(test, feature = "test-helpers"))]
pub mod inner {
    use crate::memory::notes::indexer::NoteIndexer;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    use crate::memory::events::handler::MemoryCommandHandler;

    /// Build a `MemoryCommandHandler` wired to a real on-disk
    /// `NoteIndexer` and an in-memory `StateDatabase`. The temp
    /// directories must be kept alive by binding them in the test
    /// frame; dropping the returned `TempDir` (which guards the
    /// notes filesystem) tears down the on-disk indexer backing
    /// store.
    ///
    /// Returns `(memory_dir_guard, handler)` where `memory_dir_guard`
    /// must outlive every operation issued on `handler` (the indexer
    /// points at it via `memory_dir`).
    pub async fn make_handler_with_indexer() -> (tempfile::TempDir, MemoryCommandHandler) {
        let (_scratch, db_path) = crate::utils::scratch::scratch_root();
        let memory_dir = tempfile::TempDir::new().expect("tempdir");
        let db = Arc::new(SqliteMemoryBackend::new(&db_path).expect("memory backend"));
        let indexer = Arc::new(NoteIndexer::new(
            memory_dir.path().to_path_buf(),
            Arc::clone(&db),
        ));
        let state_db = Arc::new(
            crate::resilience::database::StateDatabase::in_memory().expect("in-memory state db"),
        );
        (
            memory_dir,
            MemoryCommandHandler::new(state_db).with_note_indexer(indexer),
        )
    }
}
