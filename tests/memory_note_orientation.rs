//! End-to-end: bootstrap → write 5 notes → rebuild index → assert all three
//! files on disk + orientation snapshot populated.

use alephcore::memory::notes::orientation::TokenBudget;
use alephcore::memory::notes::orientation::{FsNoteOrientation, NoteOrientation};
use alephcore::memory::notes::{KnowledgeNote, NoteIndexer};
use alephcore::memory::store::sqlite::SqliteMemoryBackend;
use std::sync::Arc;

#[tokio::test]
async fn orientation_layer_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let orient: Arc<dyn NoteOrientation> = Arc::new(FsNoteOrientation::new(
        dir.path().join("note"),
        backend.clone(),
    ));
    orient.bootstrap("default").await.unwrap();

    let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

    for (cat, name) in [
        ("learning", "rust-async"),
        ("learning", "tokio"),
        ("preference", "editor"),
        ("project", "aleph"),
        ("tool", "ast-grep"),
    ] {
        let note = KnowledgeNote {
            title: name.into(),
            category: cat.into(),
            tags: vec![],
            facts: vec![format!("first fact of {name}")],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            ..Default::default()
        };
        indexer.write_note("default", cat, &note).await.unwrap();
    }

    // The indexer has no orientation handle at all — `NoteOrientation` is
    // driven explicitly, never as a write-path side effect. So we populate
    // SQLite via full_rebuild and then rebuild the index ourselves, which is
    // exactly how production reaches `rebuild_index` (it reads
    // store.list_notes()).
    indexer.full_rebuild("default").await.unwrap();

    orient.rebuild_index("default").await.unwrap();

    let base = dir.path().join("note/default");
    assert!(base.join("SCHEMA.md").exists(), "SCHEMA.md missing");
    assert!(base.join("index.md").exists(), "index.md missing");
    assert!(base.join("log.md").exists(), "log.md missing");

    let index = tokio::fs::read_to_string(base.join("index.md"))
        .await
        .unwrap();
    for name in ["rust-async", "tokio", "editor", "aleph", "ast-grep"] {
        assert!(index.contains(name), "index.md missing {name}: {index}");
    }

    let snap = orient
        .read_snapshot("default", TokenBudget::default())
        .await
        .unwrap();
    // read_snapshot returns the compacted schema (policy sections only — the
    // "# Memory Schema" H1 and ## Domain are stripped by compact_for_prompt).
    assert!(snap.schema_text.contains("## Tag Taxonomy"));
    assert!(snap.index_text.contains("## learning (2)"));
    assert!(snap.recent_log_tail.contains("bootstrap"));
}
