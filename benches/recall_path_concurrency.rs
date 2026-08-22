//! Recall-path concurrency benchmark.
//
// Measures the wall-clock cost of the three recall/UI hot paths that
//! Risk 4 part 2 migrated to `spawn_blocking`:
//   * `search_notes_fts`  (recall query: multi-JOIN + FTS5 MATCH)
//   * `list_notes`        (UI list: notes_index scan + subquery)
//   * `find_by_filename`  (UI search autocomplete: filename lookup)
// across two regimes:
//   * sequential: one tokio task, 100 queries back-to-back
//   * concurrent: N tokio tasks, 100 queries each
//
// The ratio (sequential_total / concurrent_total × N) approximates the
// executor-blocking savings from spawn_blocking: if the helper were a
// sync lock, concurrent total would be ≈ sequential total (serialised
// behind the Mutex). With spawn_blocking, concurrent total should be
// well below sequential total as long as N ≤ available blocking-pool
// threads (default 512).
//
// Methodology:
//   1. Build a fresh in-memory backend + NoteIndexer.
//   2. Insert 100 notes across 10 distinct titles (10 per title) so every
//      query has at least one hit (no zero-result bias).
//   3. Run the regimes; report elapsed + per-query latency.
//   4. Sanity-check that every query returns at least one row.
//
// Invoke via:
//   cargo bench --bench recall_path_concurrency --release
// or, for a quick smoke test without criterion's measurement loop:
//   cargo run --release --bin recall_path_concurrency

use std::sync::Arc;
use std::time::Instant;

use alephcore::memory::notes::indexer::NoteIndexer;
use alephcore::memory::notes::store::NoteStore;
use alephcore::memory::notes::KnowledgeNote;
use alephcore::memory::store::SqliteMemoryBackend;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    println!("=== recall-path concurrency benchmark ===");
    println!();

    // Fixture: 100 notes, 10 distinct titles x 10 each.
    const N_NOTES: usize = 100;
    const TITLES: [&str; 10] = [
        "topic-rust",
        "topic-async",
        "topic-spawn",
        "topic-blocking",
        "topic-mutex",
        "topic-concurrency",
        "topic-runtime",
        "topic-executor",
        "tokio-task",
        "sql-fts",
    ];

    let dir = tempfile::TempDir::new().expect("tempdir");
    let db_path = dir.path().join("memory.db");
    let store = Arc::new(SqliteMemoryBackend::new(&db_path).expect("backend"));
    let indexer = Arc::new(NoteIndexer::new(
        dir.path().to_path_buf(),
        Arc::clone(&store),
    ));
    indexer.ensure_dirs("default").await.expect("ensure_dirs");

    for i in 0..N_NOTES {
        let title = TITLES[i % TITLES.len()];
        let note = KnowledgeNote {
            title: title.to_string(),
            category: "preference".to_string(),
            tags: vec![],
            facts: vec![format!("Recall fixture body #{i}: {title}")],
            links: vec![],
            body: Some(format!(
                "## {title}\nThis note covers async / await, spawn_blocking, and tokio runtime."
            )),
            content_hash: String::new(),
            ..Default::default()
        };
        indexer
            .write_note("default", "preference", &note)
            .await
            .expect("write_note");
    }

    // Sanity-check: every query must hit at least one row.
    for title in &TITLES {
        let hits = store
            .search_notes_fts(title, "default", 20)
            .await
            .expect("sanity search");
        assert!(
            !hits.is_empty(),
            "fixture invariant violated: search for {title:?} returned no rows"
        );
    }

    // ---- Sequential regime ----
    println!("Sequential (1 task × 100 queries each):");
    let seq_search = bench_search_fts_seq(Arc::clone(&store), 100).await;
    let seq_list = bench_list_notes_seq(Arc::clone(&store), 100).await;
    let seq_find = bench_find_by_filename_seq(Arc::clone(&store), 100).await;
    println!(
        "  search_notes_fts     : {} ms total ({} us/query)",
        seq_search.0, seq_search.1
    );
    println!(
        "  list_notes           : {} ms total ({} us/query)",
        seq_list.0, seq_list.1
    );
    println!(
        "  find_by_filename     : {} ms total ({} us/query)",
        seq_find.0, seq_find.1
    );
    println!();

    // ---- Concurrent regime ----
    println!("Concurrent (32 tasks × 100 queries each, 4 tokio workers):");
    let cc_search = bench_search_fts_concurrent(Arc::clone(&store), 32, 100).await;
    let cc_list = bench_list_notes_concurrent(Arc::clone(&store), 32, 100).await;
    let cc_find = bench_find_by_filename_concurrent(Arc::clone(&store), 32, 100).await;
    println!(
        "  search_notes_fts     : {} ms total ({:.2} us/query)",
        cc_search.0, cc_search.1
    );
    println!(
        "  list_notes           : {} ms total ({:.2} us/query)",
        cc_list.0, cc_list.1
    );
    println!(
        "  find_by_filename     : {} ms total ({:.2} us/query)",
        cc_find.0, cc_find.1
    );
    println!();

    // ---- Speedup ----
    let seq_search_total = seq_search.0 as f64;
    let cc_search_total = cc_search.0 as f64;
    let search_speedup = if cc_search_total > 0.0 {
        seq_search_total / cc_search_total
    } else {
        f64::INFINITY
    };
    let list_speedup = if cc_list.0 > 0 {
        seq_list.0 as f64 / cc_list.0 as f64
    } else {
        f64::INFINITY
    };
    let find_speedup = if cc_find.0 > 0 {
        seq_find.0 as f64 / cc_find.0 as f64
    } else {
        f64::INFINITY
    };
    println!("Concurrency speedup (sequential total / concurrent total):");
    println!(
        "  search_notes_fts     : {:.2}x  ({}-task parallelism)",
        search_speedup, 32
    );
    println!(
        "  list_notes           : {:.2}x  ({}-task parallelism)",
        list_speedup, 32
    );
    println!(
        "  find_by_filename     : {:.2}x  ({}-task parallelism)",
        find_speedup, 32
    );

    println!();
    println!("Interpretation:");
    println!("  SqliteMemoryBackend owns a single Arc<Mutex<Connection>>.");
    println!("  SQLite itself is single-connection / single-writer, so concurrent");
    println!("  queries serialize through the Mutex regardless of spawn_blocking.");
    println!("  Spawn_blocking's actual benefit is keeping the calling tokio");
    println!("  worker thread free during the query so other tasks (HTTP handlers,");
    println!("  dream cycle, channel health) keep making progress.");
    println!("  What we expect to see in this benchmark:");
    println!("    - sequential_total / concurrent_total ~ 1/(32) (linear scaling)");
    println!("      because the SQL itself is the bottleneck, not the lock.");
    println!("    - per-query latency in concurrent ~= per-query latency in");
    println!("      sequential (because Connection is serial anyway).");
    println!("  If concurrent_total were MUCH higher than sequential_total *");
    println!("  queries_concurrent / queries_sequential, that would mean the");
    println!("  sync lock was being held on the worker thread (the regression");
    println!("  we'd want to catch).");
    println!();
    println!("=== done ===");
}

async fn bench_search_fts_seq(store: Arc<SqliteMemoryBackend>, queries: usize) -> (u128, u128) {
    let start = Instant::now();
    for i in 0..queries {
        let q = topic_for(i);
        let hits = store
            .search_notes_fts(&q, "default", 20)
            .await
            .expect("search_notes_fts");
        assert!(!hits.is_empty(), "seq search {q:?} returned no rows");
    }
    let total = start.elapsed().as_millis();
    let per_query_us = start.elapsed().as_micros() / queries as u128;
    (total, per_query_us)
}

async fn bench_list_notes_seq(store: Arc<SqliteMemoryBackend>, queries: usize) -> (u128, u128) {
    let start = Instant::now();
    for _ in 0..queries {
        let entries = store.list_notes("default").await.expect("list_notes");
        assert!(!entries.is_empty(), "seq list_notes returned no rows");
    }
    let total = start.elapsed().as_millis();
    let per_query_us = start.elapsed().as_micros() / queries as u128;
    (total, per_query_us)
}

async fn bench_find_by_filename_seq(
    store: Arc<SqliteMemoryBackend>,
    queries: usize,
) -> (u128, u128) {
    let start = Instant::now();
    for i in 0..queries {
        let q = topic_for(i);
        let paths = store
            .find_by_filename(&q, "default")
            .await
            .expect("find_by_filename");
        assert!(
            !paths.is_empty(),
            "seq find_by_filename {q:?} returned no rows"
        );
    }
    let total = start.elapsed().as_millis();
    let per_query_us = start.elapsed().as_micros() / queries as u128;
    (total, per_query_us)
}

async fn bench_search_fts_concurrent(
    store: Arc<SqliteMemoryBackend>,
    tasks: usize,
    queries_per_task: usize,
) -> (u128, f64) {
    let total_queries = tasks * queries_per_task;
    let start = Instant::now();
    let mut handles = Vec::with_capacity(tasks);
    for _ in 0..tasks {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for i in 0..queries_per_task {
                let q = topic_for(i);
                let hits = store
                    .search_notes_fts(&q, "default", 20)
                    .await
                    .expect("concurrent search");
                assert!(!hits.is_empty());
            }
        }));
    }
    for h in handles {
        h.await.expect("task join");
    }
    let elapsed = start.elapsed();
    let total_ms = elapsed.as_millis();
    let per_query_us = elapsed.as_micros() as f64 / total_queries as f64;
    (total_ms, per_query_us)
}

async fn bench_list_notes_concurrent(
    store: Arc<SqliteMemoryBackend>,
    tasks: usize,
    queries_per_task: usize,
) -> (u128, f64) {
    let total_queries = tasks * queries_per_task;
    let start = Instant::now();
    let mut handles = Vec::with_capacity(tasks);
    for _ in 0..tasks {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for _ in 0..queries_per_task {
                let entries = store.list_notes("default").await.expect("list");
                assert!(!entries.is_empty());
            }
        }));
    }
    for h in handles {
        h.await.expect("task join");
    }
    let elapsed = start.elapsed();
    let total_ms = elapsed.as_millis();
    let per_query_us = elapsed.as_micros() as f64 / total_queries as f64;
    (total_ms, per_query_us)
}

async fn bench_find_by_filename_concurrent(
    store: Arc<SqliteMemoryBackend>,
    tasks: usize,
    queries_per_task: usize,
) -> (u128, f64) {
    let total_queries = tasks * queries_per_task;
    let start = Instant::now();
    let mut handles = Vec::with_capacity(tasks);
    for _ in 0..tasks {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for i in 0..queries_per_task {
                let q = topic_for(i);
                let paths = store.find_by_filename(&q, "default").await.expect("find");
                assert!(!paths.is_empty());
            }
        }));
    }
    for h in handles {
        h.await.expect("task join");
    }
    let elapsed = start.elapsed();
    let total_ms = elapsed.as_millis();
    let per_query_us = elapsed.as_micros() as f64 / total_queries as f64;
    (total_ms, per_query_us)
}

fn topic_for(i: usize) -> String {
    const TITLES: [&str; 10] = [
        "topic-rust",
        "topic-async",
        "topic-spawn",
        "topic-blocking",
        "topic-mutex",
        "topic-concurrency",
        "topic-runtime",
        "topic-executor",
        "tokio-task",
        "sql-fts",
    ];
    TITLES[i % TITLES.len()].to_string()
}
