//! Phase B B3 benchmark — measures `NoteIndexer::full_rebuild` on 1000 notes
//! split across 5 categories (preference / skill / reference / plan / learning).
//!
//! Methodology:
//!   1. Lay 1000 notes on disk via `write_note` (this also populates SQLite).
//!   2. Drop the backend and delete the SQLite file so the rebuild is cold —
//!      every file must be parsed and re-indexed (no skip-by-hash).
//!   3. Reopen, time `full_rebuild`, print the elapsed wall-clock.
//!
//! To compare serial vs parallel, hardcode the parallelism probe to `1` in
//! `indexer.rs::full_rebuild` and rerun. Do not commit that change.

use std::sync::Arc;
use std::time::Instant;

use alephcore::memory::notes::{KnowledgeNote, NoteIndexer};
use alephcore::memory::store::SqliteMemoryBackend;

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join(format!("aleph_bench_rebuild_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let db_path = dir.join("db.sqlite");
    let store = Arc::new(SqliteMemoryBackend::new(&db_path).expect("open backend"));
    let indexer = NoteIndexer::new(dir.clone(), store.clone());
    indexer.ensure_dirs("default").await.unwrap();

    let cats = ["preference", "skill", "reference", "plan", "learning"];
    for i in 0..1000 {
        let cat = cats[i % cats.len()];
        let note = KnowledgeNote {
            title: format!("n-{i}"),
            category: cat.into(),
            facts: vec![format!("body {i}")],
            content_hash: String::new(),
            ..Default::default()
        };
        indexer.write_note("default", cat, &note).await.unwrap();
    }

    // Wipe SQLite state so the rebuild is a true cold rebuild — every file
    // must be parsed and inserted, exercising the parallel scan + write path.
    drop(indexer);
    drop(store);
    let _ = std::fs::remove_file(&db_path);

    let store = Arc::new(SqliteMemoryBackend::new(&db_path).expect("reopen backend"));
    let indexer = NoteIndexer::new(dir.clone(), store.clone());
    indexer.ensure_dirs("default").await.unwrap();

    let start = Instant::now();
    let stats = indexer.full_rebuild("default").await.unwrap();
    let elapsed = start.elapsed();

    println!(
        "BENCH full_rebuild_1000: {:?} (indexed={}, skipped={}, errors={})",
        elapsed, stats.indexed, stats.skipped, stats.errors
    );

    let _ = std::fs::remove_dir_all(&dir);
}
