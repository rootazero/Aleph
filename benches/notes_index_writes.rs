//! Phase B baseline benchmark — measures write amplification on
//! `notes_links` and `notes_fts` when the same set of notes is re-indexed
//! repeatedly without content changes.
//!
//! Methodology:
//!   1. Insert 100 notes once (warm-up — round 0, not measured)
//!   2. Re-insert the SAME 100 notes for 5 rounds (rounds 0..5, all timed)
//!   3. Average rounds 1..5 to get steady-state per-round time
//!
//! The `steady_state_avg_per_round` line is the pre-fix baseline.
//! Tasks B1.2 (set-diff links) and B1.3 (FTS hash gate) must reduce this
//! by >=70% to clear the Phase B verification gate.

use std::sync::Arc;
use std::time::{Duration, Instant};

use alephcore::memory::notes::store::NoteStore;
use alephcore::memory::notes::KnowledgeNote;
use alephcore::memory::store::SqliteMemoryBackend;

fn make_note(i: usize) -> KnowledgeNote {
    KnowledgeNote {
        title: format!("note-{i}"),
        category: "preference".into(),
        facts: (0..10).map(|j| format!("fact {i}.{j}")).collect(),
        links: (0..5).map(|j| format!("target-{}", (i + j) % 50)).collect(),
        content_hash: format!("h-{i}"),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    // Use a unique temp path per process so concurrent runs do not collide.
    let temp = std::env::temp_dir().join(format!("aleph_bench_writes_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&temp);

    let store = Arc::new(SqliteMemoryBackend::new(&temp).expect("open backend"));

    // Phase 1: initial population (not timed — establishes the row set).
    for i in 0..100 {
        store
            .index_note(&make_note(i), "default", "preference")
            .await
            .expect("initial index_note");
    }

    // Rounds 0..5: re-index the SAME 100 notes with unchanged content.
    // Every round triggers full DELETE+INSERT into notes_links + notes_fts
    // under the current implementation — that is the write amplification
    // we are measuring.
    let mut totals: Vec<Duration> = Vec::with_capacity(5);
    for round in 0..5 {
        let start = Instant::now();
        for i in 0..100 {
            store
                .index_note(&make_note(i), "default", "preference")
                .await
                .expect("re-index_note");
        }
        let elapsed = start.elapsed();
        totals.push(elapsed);
        println!("round {round}: {elapsed:?}");
    }

    // Average rounds 1..5 (drop round 0 to skip any first-round JIT/cache warmup).
    let avg = totals[1..].iter().copied().sum::<Duration>() / 4;
    println!("BENCH steady_state_avg_per_round: {avg:?}");

    // Best-effort cleanup; non-fatal if the file is locked by other readers.
    let _ = std::fs::remove_file(&temp);
}
