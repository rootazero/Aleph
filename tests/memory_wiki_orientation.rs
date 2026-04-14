//! End-to-end: bootstrap → write 5 notes → rebuild index → assert all three
//! files on disk + orientation snapshot populated.

use alephcore::memory::notes::{KnowledgeNote, NoteIndexer};
use alephcore::memory::store::sqlite::SqliteMemoryBackend;
use alephcore::memory::wiki::orientation::{FsWikiOrientation, WikiOrientation};
use alephcore::memory::wiki::types::TokenBudget;
use std::sync::Arc;

#[tokio::test]
async fn orientation_layer_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let orient: Arc<dyn WikiOrientation> = Arc::new(FsWikiOrientation::new(
        dir.path().join("note"),
        backend.clone(),
    ));
    orient.bootstrap("default").await.unwrap();

    let indexer =
        NoteIndexer::new(dir.path().join("note"), backend.clone()).with_wiki(orient.clone());

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
        };
        indexer.write_note("default", cat, &note).await.unwrap();
    }

    // write_note only writes to disk and notifies the wiki invalidation hook.
    // We must populate SQLite via full_rebuild so that WikiOrientation::rebuild_index
    // (which reads from store.list_notes()) finds all five notes.
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
    assert!(snap.schema_text.contains("# Memory Schema"));
    assert!(snap.index_text.contains("## learning (2)"));
    assert!(snap.recent_log_tail.contains("bootstrap"));
}
