# Aleph Note Layer — Phase B: Performance & Cadence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce write amplification on notes_links/notes_fts, add composite index for filename lookups, parallelize full_rebuild, drive index.md refresh from ingest commits (not dream cycle alone), and batch embedding generation.

**Architecture:** Touches `src/memory/store/sqlite/{notes.rs, schema.rs}`, `src/memory/notes/{indexer.rs, orientation/}`, `src/memory/notes/ingest/apply.rs`, `src/memory/dreaming/stages/{feedback_distill.rs, index_refresher.rs}`, and `src/memory/embedding_manager.rs`. No data-format changes. Phase B depends on Phase A's `to_raw` column.

**Tech Stack:** Rust 2021, rusqlite + sqlite-vec, tokio (JoinSet, Semaphore, time::pause), criterion (benches).

**Spec:** `docs/superpowers/specs/2026-05-03-aleph-note-layer-llm-wiki-optimization-design.md` §3 (Phase B). All B1–B6 sub-section IDs map directly to task headings below.

**Verification gate:** the new bench shows ≥70% write reduction (B1), ≥4× speedup on full_rebuild_1000_notes (B3), `EXPLAIN QUERY PLAN` confirms composite index usage (B2), integration tests pass for ingest-driven index.md refresh (B4) and embedding flush (B5), and Phase A regressions stay green.

---

## Task 1 (B1.1): Bench harness for write counting

**Files:**
- Create: `benches/notes_index_writes.rs`
- Modify: `Cargo.toml` (add bench entry under `[[bench]]`)

- [ ] **Step 1: Add bench definition to Cargo.toml**

In the workspace `Cargo.toml` (or the alephcore crate `Cargo.toml`, depending on the project's bench layout), add:

```toml
[[bench]]
name = "notes_index_writes"
harness = false
```

If `criterion` is not yet a dev-dependency, add:

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }
```

- [ ] **Step 2: Write the benchmark file**

Create `benches/notes_index_writes.rs`:

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use alephcore::memory::notes::KnowledgeNote;
use alephcore::memory::notes::store::NoteStore;
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

fn count_writes(conn: &rusqlite::Connection) -> i64 {
    conn.query_row(
        "SELECT total_changes()",
        [],
        |r| r.get(0),
    ).unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let temp = std::env::temp_dir().join("aleph_bench_writes");
    let _ = std::fs::remove_file(&temp);
    let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

    // Phase 1: initial population
    for i in 0..100 {
        store.index_note(&make_note(i), "default", "preference").await.unwrap();
    }

    // Round 2-5: re-index unchanged notes
    let mut totals = vec![];
    for round in 0..5 {
        let start = Instant::now();
        for i in 0..100 {
            store.index_note(&make_note(i), "default", "preference").await.unwrap();
        }
        let elapsed = start.elapsed();
        totals.push(elapsed);
        println!("round {round}: {:?}", elapsed);
    }

    let avg = totals[1..].iter().copied().sum::<Duration>() / 4;
    println!("BENCH steady_state_avg_per_round: {:?}", avg);
}
```

- [ ] **Step 3: Run baseline bench (pre-fix)**

```bash
cargo bench -p alephcore --bench notes_index_writes -- --nocapture
```

Record the printed `steady_state_avg_per_round` — this is the pre-B1 baseline.

- [ ] **Step 4: Commit the bench harness**

```bash
git add Cargo.toml benches/notes_index_writes.rs
git commit -m "bench: add notes_index_writes harness for B1 write-amp measurement"
```

---

## Task 2 (B1.2): Set-diff upsert for notes_links

**Files:**
- Modify: `src/memory/store/sqlite/notes.rs:88-105` (link insert block)

- [ ] **Step 1: Write a unit test asserting unchanged-link no-op**

Add to `src/memory/store/sqlite/notes.rs` `mod tests`:

```rust
#[tokio::test]
async fn reindex_unchanged_links_no_writes() {
    let temp = std::env::temp_dir().join(format!("aleph_diff_{}", uuid::Uuid::new_v4()));
    let db = SqliteMemoryBackend::new(&temp).unwrap();

    let note = crate::memory::notes::KnowledgeNote {
        title: "x".into(),
        category: "preference".into(),
        facts: vec!["body".into()],
        links: vec!["a".into(), "b".into(), "c".into()],
        content_hash: "h0".into(),
        ..Default::default()
    };

    db.index_note(&note, "default", "preference").await.unwrap();

    let conn_changes_before = {
        let conn = db.conn().lock().unwrap();
        conn.query_row::<i64, _, _>("SELECT total_changes()", [], |r| r.get(0)).unwrap()
    };

    db.index_note(&note, "default", "preference").await.unwrap();

    let conn_changes_after = {
        let conn = db.conn().lock().unwrap();
        conn.query_row::<i64, _, _>("SELECT total_changes()", [], |r| r.get(0)).unwrap()
    };

    let delta = conn_changes_after - conn_changes_before;
    // index_note still touches notes_index (1 upsert) + notes_fts (1 delete + 1 insert).
    // Links must contribute 0 because all three are unchanged.
    assert!(delta <= 3, "expected ≤3 writes from index/fts only, got {delta}");
}
```

If the backend struct does not expose a `conn()` accessor, add a `#[cfg(test)] pub fn conn(&self) -> &Mutex<rusqlite::Connection>` helper next to the struct definition.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::reindex_unchanged_links_no_writes
```
Expected: fails — current code DELETEs and re-INSERTs every link.

- [ ] **Step 3: Replace bulk delete+insert with set-diff**

Replace the link block in `src/memory/store/sqlite/notes.rs` (around lines 92-105):

```rust
// Set-diff upsert: only INSERT added rows, DELETE removed rows; keep intersection untouched.
let new_pairs: std::collections::HashSet<(String, String)> = note
    .links
    .iter()
    .map(|raw_target| {
        let resolved = if raw_target.contains('/') {
            raw_target.clone()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2 LIMIT 2",
                )
                .expect("prepare resolve filename");
            let paths: Vec<String> = stmt
                .query_map(params![agent_id, raw_target], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            if paths.len() == 1 { paths[0].clone() } else { raw_target.clone() }
        };
        (raw_target.clone(), resolved)
    })
    .collect();

let mut existing_stmt = conn
    .prepare("SELECT to_raw, to_note FROM notes_links WHERE agent_id = ?1 AND from_note = ?2")
    .map_err(|e| AlephError::config(format!("set_diff prepare: {e}")))?;
let existing: std::collections::HashSet<(String, String)> = existing_stmt
    .query_map(params![agent_id, path], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map_err(|e| AlephError::config(format!("set_diff scan: {e}")))?
    .filter_map(|r| r.ok())
    .collect();
drop(existing_stmt);

// DELETE rows that are no longer present.
for (to_raw, to_note) in existing.difference(&new_pairs) {
    conn.execute(
        "DELETE FROM notes_links WHERE agent_id = ?1 AND from_note = ?2 AND to_raw = ?3 AND to_note = ?4",
        params![agent_id, path, to_raw, to_note],
    )
    .map_err(|e| AlephError::config(format!("set_diff delete: {e}")))?;
}
// INSERT rows that are newly added.
for (to_raw, to_note) in new_pairs.difference(&existing) {
    conn.execute(
        "INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note, to_raw) VALUES (?1, ?2, ?3, ?4)",
        params![agent_id, path, to_note, to_raw],
    )
    .map_err(|e| AlephError::config(format!("set_diff insert: {e}")))?;
}
```

- [ ] **Step 4: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::reindex_unchanged_links_no_writes
```
Expected: pass.

- [ ] **Step 5: Run wider regression**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes memory::notes
```
Expected: all green (Phase A's `incoming_links_resolve_mixed_link_forms` and `lint_resolves_pending_links` must still pass).

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/sqlite/notes.rs
git commit -m "perf(notes): set-diff upsert for notes_links removes per-reindex write storm"
```

---

## Task 3 (B1.3): Skip notes_fts rebuild when body unchanged

**Files:**
- Modify: `src/memory/store/sqlite/notes.rs:107-118` (FTS write block)

- [ ] **Step 1: Add a unit test**

Add to `mod tests`:

```rust
#[tokio::test]
async fn reindex_same_body_skips_fts_rewrite() {
    let temp = std::env::temp_dir().join(format!("aleph_fts_{}", uuid::Uuid::new_v4()));
    let db = SqliteMemoryBackend::new(&temp).unwrap();
    let note = crate::memory::notes::KnowledgeNote {
        title: "x".into(),
        category: "preference".into(),
        facts: vec!["unchanged body".into()],
        content_hash: "h0".into(),
        ..Default::default()
    };
    db.index_note(&note, "default", "preference").await.unwrap();

    let before: i64 = {
        let conn = db.conn().lock().unwrap();
        conn.query_row("SELECT total_changes()", [], |r| r.get(0)).unwrap()
    };

    // Same body, different content_hash (frontmatter changed)
    let mut note2 = note.clone();
    note2.content_hash = "h1".into();
    db.index_note(&note2, "default", "preference").await.unwrap();

    let after: i64 = {
        let conn = db.conn().lock().unwrap();
        conn.query_row("SELECT total_changes()", [], |r| r.get(0)).unwrap()
    };

    // notes_index UPDATE = 1; notes_fts must not contribute.
    assert!(after - before <= 1, "expected only notes_index update, got {} writes", after - before);
}
```

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::reindex_same_body_skips_fts_rewrite
```
Expected: fail.

- [ ] **Step 3: Compare body hash before rewriting FTS**

Replace the FTS block (around lines 107-118):

```rust
// Skip notes_fts rebuild if body text is unchanged.
let body = note.body_text();
let body_hash = {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    format!("{:x}", h.finalize())
};

let prev_body_hash: Option<String> = conn
    .query_row(
        "SELECT content_hash FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
        params![agent_id, path],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| AlephError::config(format!("fts meta lookup: {e}")))?;

if prev_body_hash.as_deref() != Some(&body_hash) {
    conn.execute(
        "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
        params![path, agent_id],
    )
    .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;
    conn.execute(
        "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
        params![path, filename, body, agent_id],
    )
    .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;
    conn.execute(
        "INSERT INTO notes_fts_meta (agent_id, path, content_hash) VALUES (?1, ?2, ?3)
         ON CONFLICT(agent_id, path) DO UPDATE SET content_hash = excluded.content_hash",
        params![agent_id, path, body_hash],
    )
    .map_err(|e| AlephError::config(format!("fts meta upsert: {e}")))?;
}
```

Add the supporting table to `src/memory/store/sqlite/schema.rs` next to the other notes DDL:

```rust
const NOTES_FTS_META_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_fts_meta (
    agent_id     TEXT NOT NULL,
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
";
```

Wire it into `init_schema`:

```rust
conn.execute_batch(NOTES_FTS_META_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create notes_fts_meta: {e}")))?;
```

- [ ] **Step 4: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::store::sqlite::notes::tests::reindex_same_body_skips_fts_rewrite
```
Expected: pass.

- [ ] **Step 5: Re-run baseline bench**

```bash
cargo bench -p alephcore --bench notes_index_writes -- --nocapture
```
Compare new `steady_state_avg_per_round` against the pre-B1 baseline recorded in Task 1. Expected: ≥70% reduction.

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/sqlite/notes.rs src/memory/store/sqlite/schema.rs
git commit -m "perf(notes): skip notes_fts rebuild when body unchanged via content hash"
```

---

## Task 4 (B2): Composite index `(agent_id, filename)`

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` (next to `idx_notes_filename`)

- [ ] **Step 1: Write the EXPLAIN QUERY PLAN test**

Add to `src/memory/store/sqlite/schema.rs` `mod tests`:

```rust
#[test]
fn find_by_filename_uses_composite_index() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();

    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2",
            rusqlite::params!["default", "rust"],
            |r| r.get::<_, String>(3),
        )
        .unwrap();
    assert!(plan.contains("idx_notes_filename_agent"), "plan was: {plan}");
}
```

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema::tests::find_by_filename_uses_composite_index
```
Expected: fails — composite index does not exist.

- [ ] **Step 3: Add composite index to DDL**

Locate the `notes_index` DDL block in `src/memory/store/sqlite/schema.rs` and append after the existing indexes:

```rust
"CREATE INDEX IF NOT EXISTS idx_notes_filename_agent ON notes_index(agent_id, filename);"
```

(Do not remove `idx_notes_filename` yet — keep one release for backward compatibility.)

- [ ] **Step 4: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "perf(notes): add composite index (agent_id, filename) on notes_index"
```

---

## Task 5 (B3): Parallel `full_rebuild` per category

**Files:**
- Modify: `src/memory/notes/indexer.rs:116` (`full_rebuild`)
- Add: `num_cpus` to dev-dependencies if not already present

- [ ] **Step 1: Write a parity test**

Add to `src/memory/notes/indexer.rs` `mod tests`:

```rust
#[tokio::test]
async fn full_rebuild_parallel_matches_serial_results() {
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;
    use std::sync::Arc;

    let dir = std::env::temp_dir().join(format!("aleph_pll_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let store = Arc::new(SqliteMemoryBackend::new(&dir.join("db.sqlite")).unwrap());
    let indexer = NoteIndexer::new(dir.clone(), store.clone(), None);
    indexer.ensure_dirs("default").await.unwrap();

    // Lay 50 notes spread across 5 categories
    for cat in &["preference", "skill", "reference", "plan", "learning"] {
        for i in 0..10 {
            let note = KnowledgeNote {
                title: format!("n{cat}-{i}"),
                category: (*cat).into(),
                facts: vec![format!("fact {i}")],
                content_hash: String::new(),
                ..Default::default()
            };
            indexer.write_note("default", cat, &note).await.unwrap();
        }
    }

    let stats = indexer.full_rebuild("default").await.unwrap();
    assert_eq!(stats.indexed + stats.skipped, 50);
    assert_eq!(stats.errors, 0);

    let listed = store.list_notes("default").await.unwrap();
    assert_eq!(listed.len(), 50);
}
```

- [ ] **Step 2: Run test — currently passes serially; now refactor with parity**

```bash
cargo test -p alephcore --lib memory::notes::indexer::tests::full_rebuild_parallel_matches_serial_results
```
Expected: pass with current implementation. The refactor must preserve this property.

- [ ] **Step 3: Refactor `full_rebuild` to use `JoinSet`**

Replace the body of `full_rebuild` at `src/memory/notes/indexer.rs:116`:

```rust
pub async fn full_rebuild(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    self.ensure_dirs(agent_id).await?;

    let limit = num_cpus::get().max(1);
    let sem = Arc::new(Semaphore::new(limit));
    let mut set: JoinSet<Result<IndexStats, AlephError>> = JoinSet::new();

    for category in CATEGORY_DIRS {
        let agent_id = agent_id.to_string();
        let category = (*category).to_string();
        let memory_dir = self.memory_dir.clone();
        let store = self.store.clone();
        let sem = sem.clone();

        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            let dir = memory_dir.join(&agent_id).join(&category);
            let mut local = IndexStats::default();

            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => return Ok(local),
            };

            while let Some(entry) = entries.next_entry().await
                .map_err(|e| AlephError::config(format!("read_dir: {e}")))?
            {
                let path = entry.path();
                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    match index_one_file(&path, &agent_id, &category, store.clone()).await {
                        Ok(IndexOutcome::Indexed) => local.indexed += 1,
                        Ok(IndexOutcome::Skipped) => local.skipped += 1,
                        Err(e) => {
                            tracing::warn!(?path, error = %e, "full_rebuild: file failed");
                            local.errors += 1;
                        }
                    }
                }
            }
            Ok(local)
        });
    }

    let mut total = IndexStats::default();
    while let Some(joined) = set.join_next().await {
        match joined.map_err(|e| AlephError::config(format!("join: {e}")))? {
            Ok(s) => {
                total.indexed += s.indexed;
                total.skipped += s.skipped;
                total.errors  += s.errors;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}
```

Extract a helper `index_one_file` near the top of the same module (private):

```rust
enum IndexOutcome { Indexed, Skipped }

async fn index_one_file<S: NoteStore + ?Sized + 'static>(
    path: &std::path::Path,
    agent_id: &str,
    category: &str,
    store: std::sync::Arc<S>,
) -> Result<IndexOutcome, AlephError> {
    let content = tokio::fs::read_to_string(path).await
        .map_err(|e| AlephError::config(format!("read {}: {e}", path.display())))?;
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let note = crate::memory::notes::KnowledgeNote::from_markdown(&title, &content)?;

    let key_path = format!("{category}/{title}");
    if let Some(existing) = store.get_note_index(&key_path, agent_id).await? {
        if existing.content_hash == note.content_hash {
            return Ok(IndexOutcome::Skipped);
        }
    }
    store.index_note(&note, agent_id, category).await?;
    Ok(IndexOutcome::Indexed)
}
```

If `num_cpus` is not in dependencies, add it to `Cargo.toml`:

```toml
num_cpus = "1.16"
```

- [ ] **Step 4: Run parity test + existing rebuild tests**

```bash
cargo test -p alephcore --lib memory::notes::indexer
```
Expected: green; `full_rebuild_parallel_matches_serial_results` and `full_rebuild_skips_unchanged` both pass.

- [ ] **Step 5: Add a benchmark `full_rebuild_1000_notes`**

Create `benches/full_rebuild_1000_notes.rs`:

```rust
use std::sync::Arc;
use std::time::Instant;

use alephcore::memory::notes::{KnowledgeNote, NoteIndexer};
use alephcore::memory::store::SqliteMemoryBackend;

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join("aleph_bench_rebuild");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let store = Arc::new(SqliteMemoryBackend::new(&dir.join("db.sqlite")).unwrap());
    let indexer = NoteIndexer::new(dir.clone(), store.clone(), None);
    indexer.ensure_dirs("default").await.unwrap();

    for i in 0..1000 {
        let cat = ["preference", "skill", "reference", "plan", "learning"][i % 5];
        let note = KnowledgeNote {
            title: format!("n-{i}"),
            category: cat.into(),
            facts: vec![format!("body {i}")],
            content_hash: String::new(),
            ..Default::default()
        };
        indexer.write_note("default", cat, &note).await.unwrap();
    }

    // Drop SQLite indices to force a true rebuild.
    {
        // open a fresh DB instance
    }

    let start = Instant::now();
    let stats = indexer.full_rebuild("default").await.unwrap();
    let elapsed = start.elapsed();

    println!("BENCH full_rebuild_1000: {:?} (indexed={}, skipped={}, errors={})",
        elapsed, stats.indexed, stats.skipped, stats.errors);
}
```

Add to `Cargo.toml`:

```toml
[[bench]]
name = "full_rebuild_1000_notes"
harness = false
```

- [ ] **Step 6: Run the bench**

```bash
cargo bench -p alephcore --bench full_rebuild_1000_notes -- --nocapture
```
Record the elapsed time. On 8 cores, expected ≥4× speedup vs a serial baseline (run once with `JoinSet` set capacity 1 if you want to capture the serial number).

- [ ] **Step 7: Commit**

```bash
git add src/memory/notes/indexer.rs benches/full_rebuild_1000_notes.rs Cargo.toml
git commit -m "perf(notes): parallel full_rebuild via JoinSet + per-cpu semaphore"
```

---

## Task 6 (B4.1): Trait method `refresh_index_after_ingest`

**Files:**
- Modify: `src/memory/notes/orientation/mod.rs` (add trait method with default impl)
- Modify: `src/memory/notes/orientation/fs_orientation.rs` (override)

- [ ] **Step 1: Write a test asserting partial refresh emits one log line per touched category**

Add to `src/memory/notes/orientation/fs_orientation.rs` `mod tests`:

```rust
#[tokio::test]
async fn refresh_index_after_ingest_writes_only_touched_categories() {
    use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};
    let dir = std::env::temp_dir().join(format!("aleph_refresh_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let orient = FsNoteOrientation::new(dir.clone());

    let summary = IngestBatchSummary {
        agent_id: "default".into(),
        touched: vec![
            TouchedCategory { category: "preference".into(), added: 2, updated: 1 },
        ],
    };

    orient.refresh_index_after_ingest("default", &summary).await.unwrap();

    let index_md = std::fs::read_to_string(dir.join("default").join("index.md")).unwrap();
    assert!(index_md.contains("preference"), "preference category must be in index.md");
    assert!(!index_md.contains("synthesis"), "synthesis must not appear (not touched)");
}
```

If `IngestBatchSummary` and `TouchedCategory` do not yet exist, add them to `src/memory/notes/orientation/types.rs`:

```rust
#[derive(Debug, Clone)]
pub struct IngestBatchSummary {
    pub agent_id: String,
    pub touched: Vec<TouchedCategory>,
}

#[derive(Debug, Clone)]
pub struct TouchedCategory {
    pub category: String,
    pub added: u32,
    pub updated: u32,
}
```

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --lib memory::notes::orientation::fs_orientation::tests::refresh_index_after_ingest_writes_only_touched_categories
```
Expected: fails to compile (method missing).

- [ ] **Step 3: Add trait method**

In `src/memory/notes/orientation/mod.rs`, extend the `NoteOrientation` trait:

```rust
#[async_trait::async_trait]
pub trait NoteOrientation: Send + Sync {
    // ...existing methods...

    /// Refresh `index.md` for the categories touched by a single ingest batch.
    /// Default impl is a no-op so non-fs implementations don't need to opt in.
    async fn refresh_index_after_ingest(
        &self,
        agent_id: &str,
        summary: &crate::memory::notes::orientation::types::IngestBatchSummary,
    ) -> Result<(), AlephError> {
        let _ = (agent_id, summary);
        Ok(())
    }
}
```

In `src/memory/notes/orientation/fs_orientation.rs`, override:

```rust
async fn refresh_index_after_ingest(
    &self,
    agent_id: &str,
    summary: &IngestBatchSummary,
) -> Result<(), AlephError> {
    use std::collections::HashSet;
    let touched: HashSet<&str> = summary.touched.iter().map(|t| t.category.as_str()).collect();
    if touched.is_empty() {
        return Ok(());
    }
    self.regenerate_index_md_for_categories(agent_id, &touched).await
}
```

Add the helper `regenerate_index_md_for_categories` next to the existing index_md routines (the existing full-rewrite path probably lives in `index_md.rs`; refactor it to take an optional category filter and call from both whole-rewrite and the new partial path).

- [ ] **Step 4: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::notes::orientation
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/orientation/mod.rs src/memory/notes/orientation/fs_orientation.rs src/memory/notes/orientation/types.rs src/memory/notes/orientation/index_md.rs
git commit -m "feat(orientation): refresh_index_after_ingest for partial post-ingest refresh"
```

---

## Task 7 (B4.2): Wire ingest tail to `refresh_index_after_ingest`

**Files:**
- Modify: `src/memory/notes/ingest/apply.rs` (collect touched categories; call refresh)
- Modify: `src/memory/dreaming/stages/feedback_distill.rs` (same)

- [ ] **Step 1: Write integration test using `tokio::time::pause`**

Create a new file `tests/note_layer_index_refresh.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use alephcore::memory::notes::orientation::FsNoteOrientation;
use alephcore::memory::notes::ingest::{
    ingestor::DefaultCompoundIngestor, retrieve::RelatedBudget,
};
use alephcore::memory::store::SqliteMemoryBackend;

#[tokio::test(start_paused = true)]
async fn ingest_refreshes_index_md_without_dream() {
    let dir = tempfile::tempdir().unwrap();
    let orient = Arc::new(FsNoteOrientation::new(dir.path().to_path_buf()));
    let store = Arc::new(SqliteMemoryBackend::new(&dir.path().join("db")).unwrap());
    // Build a minimal ingestor with a fake LLM that emits one Note action.
    // (Use whatever fixtures the project provides for ingestor unit tests.)
    let ingestor = DefaultCompoundIngestor::new_for_test(store.clone(), orient.clone());

    ingestor.ingest_one_for_test("default", "preference", "x", "fact body").await.unwrap();

    tokio::time::advance(Duration::from_secs(1)).await;

    let index = std::fs::read_to_string(dir.path().join("default").join("index.md")).unwrap();
    assert!(index.contains("preference"));
    assert!(index.contains("x"));
}
```

If the test factories `new_for_test` / `ingest_one_for_test` do not yet exist, add minimal versions in `src/memory/notes/ingest/ingestor.rs`:

```rust
#[cfg(any(test, feature = "test-helpers"))]
impl DefaultCompoundIngestor {
    pub fn new_for_test(
        store: std::sync::Arc<dyn crate::memory::notes::store::NoteStore + Send + Sync>,
        orient: std::sync::Arc<dyn crate::memory::notes::orientation::NoteOrientation>,
    ) -> Self { /* small constructor with stub LLM */ unimplemented!() }

    pub async fn ingest_one_for_test(
        &self,
        agent_id: &str,
        category: &str,
        title: &str,
        body: &str,
    ) -> Result<(), crate::error::AlephError> { /* writes one note via the apply path */ unimplemented!() }
}
```

(Implement these against existing test fixtures; if the project already exposes a `MockIngestor`, prefer that.)

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --test note_layer_index_refresh
```
Expected: fails — apply path does not yet call `refresh_index_after_ingest`.

- [ ] **Step 3: Modify `apply.rs::ingest_batch` tail**

In `src/memory/notes/ingest/apply.rs`, locate the batch-completion site (the function that returns after writing all `NoteAction`s). Build a `IngestBatchSummary` from the actions and call:

```rust
use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};

let mut by_cat: std::collections::HashMap<String, (u32, u32)> = std::collections::HashMap::new();
for outcome in &write_outcomes {
    let entry = by_cat.entry(outcome.category.clone()).or_default();
    match outcome.kind {
        WriteKind::Created => entry.0 += 1,
        WriteKind::Updated => entry.1 += 1,
        _ => {}
    }
}
let summary = IngestBatchSummary {
    agent_id: agent_id.to_string(),
    touched: by_cat.into_iter().map(|(category, (added, updated))| TouchedCategory { category, added, updated }).collect(),
};
if let Some(orient) = self.orientation.as_ref() {
    orient.refresh_index_after_ingest(agent_id, &summary).await?;
}
```

- [ ] **Step 4: Modify `feedback_distill.rs::execute` tail**

Locate the end of `execute` in `src/memory/dreaming/stages/feedback_distill.rs`, after the last `index_file` call. Build a similar summary and call `ctx.orientation.refresh_index_after_ingest(...)` if `ctx.orientation` is `Some`.

- [ ] **Step 5: Run integration test — should pass**

```bash
cargo test -p alephcore --test note_layer_index_refresh
```
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/apply.rs src/memory/dreaming/stages/feedback_distill.rs tests/note_layer_index_refresh.rs
git commit -m "feat(orientation): wire ingest+feedback_distill tails to partial index.md refresh"
```

---

## Task 8 (B5.1): Pending-embedding queue

**Files:**
- Modify: `src/memory/embedding_manager.rs` (queue + flush)
- Modify: `src/memory/notes/indexer.rs::index_file` (push instead of generate inline)

- [ ] **Step 1: Write a unit test**

Add to `src/memory/embedding_manager.rs` `mod tests`:

```rust
#[tokio::test]
async fn pending_queue_flush_writes_all() {
    let store = std::sync::Arc::new(crate::memory::store::SqliteMemoryBackend::new(
        &std::env::temp_dir().join(format!("aleph_emb_{}", uuid::Uuid::new_v4())),
    ).unwrap());
    let mgr = EmbeddingManager::new_for_test(store.clone(), 768).await;

    mgr.push_pending("default", "preference/a", "body a").await;
    mgr.push_pending("default", "preference/b", "body b").await;
    assert_eq!(mgr.pending_len().await, 2);

    let flushed = mgr.flush_pending(64).await.unwrap();
    assert_eq!(flushed, 2);
    assert_eq!(mgr.pending_len().await, 0);

    // Both notes have vectors now.
    assert!(store.get_embedding("preference/a", "default", Some(768)).await.unwrap().is_some());
    assert!(store.get_embedding("preference/b", "default", Some(768)).await.unwrap().is_some());
}
```

- [ ] **Step 2: Run test — should fail**

```bash
cargo test -p alephcore --lib memory::embedding_manager::tests::pending_queue_flush_writes_all
```
Expected: fails — methods missing.

- [ ] **Step 3: Add queue + flush methods**

In `src/memory/embedding_manager.rs`, add:

```rust
struct PendingItem {
    agent_id: String,
    path: String,
    body: String,
}

pub struct EmbeddingManager {
    // ...existing fields...
    pending: tokio::sync::Mutex<Vec<PendingItem>>,
}

impl EmbeddingManager {
    pub async fn push_pending(&self, agent_id: &str, path: &str, body: &str) {
        let mut q = self.pending.lock().await;
        q.push(PendingItem {
            agent_id: agent_id.to_string(),
            path: path.to_string(),
            body: body.to_string(),
        });
    }

    pub async fn pending_len(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Drain up to `batch_size` items, embed them, write via NoteStore.
    /// Returns the number of items successfully flushed.
    pub async fn flush_pending(&self, batch_size: usize) -> Result<usize, AlephError> {
        let drained: Vec<PendingItem> = {
            let mut q = self.pending.lock().await;
            let take = q.len().min(batch_size);
            q.drain(..take).collect()
        };
        if drained.is_empty() {
            return Ok(0);
        }

        let bodies: Vec<&str> = drained.iter().map(|p| p.body.as_str()).collect();
        let vectors = self.embed_batch(&bodies).await?;

        let dim = self.embedding_dim();
        for (item, vec) in drained.iter().zip(vectors.iter()) {
            self.store
                .upsert_embedding(&item.path, &item.agent_id, vec, dim)
                .await?;
        }
        Ok(drained.len())
    }

    /// Spawn a background tick that flushes every 60 seconds.
    pub fn spawn_background_flush(self: std::sync::Arc<Self>) {
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_secs(60));
            iv.tick().await;
            loop {
                iv.tick().await;
                if let Err(e) = self.flush_pending(64).await {
                    tracing::warn!(error = %e, "background embedding flush failed");
                }
            }
        });
    }
}
```

(`embed_batch`, `embedding_dim`, and `new_for_test` are project-specific; if `embed_batch` does not exist, add it as a wrapper around the underlying single-call provider with explicit batching.)

- [ ] **Step 4: Replace inline embed in `index_file`**

Locate the embedding write site inside `NoteIndexer::index_file` (the existing call to `EmbeddingManager` or `store.upsert_embedding`). Replace with:

```rust
if let Some(em) = self.embedding_manager.as_ref() {
    em.push_pending(agent_id, &key_path, &note.body_text()).await;
}
```

- [ ] **Step 5: Run test — should pass**

```bash
cargo test -p alephcore --lib memory::embedding_manager
```
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/embedding_manager.rs src/memory/notes/indexer.rs
git commit -m "perf(embedding): batch pending embeddings; flush on ingest tail / 60s tick"
```

---

## Task 9 (B5.2): Trigger flushes from ingest, dream, and background

**Files:**
- Modify: `src/memory/notes/ingest/apply.rs` (call flush after `refresh_index_after_ingest`)
- Modify: `src/memory/dreaming/mod.rs` (call flush at stage tails)
- Modify: server startup (e.g. `src/bin/aleph-server/commands/start/mod.rs`) (spawn background tick)

- [ ] **Step 1: Add flush call to `apply.rs::ingest_batch` tail**

In `src/memory/notes/ingest/apply.rs`, after the `refresh_index_after_ingest` call from Task 7:

```rust
if let Some(em) = self.embedding_manager.as_ref() {
    let _ = em.flush_pending(64).await;
}
```

- [ ] **Step 2: Add flush call to dream stage tails**

In `src/memory/dreaming/mod.rs`, after each stage's `execute` returns (the existing pipeline loop), inject:

```rust
if let Some(em) = ctx.embedding_manager.as_ref() {
    let _ = em.flush_pending(64).await;
}
```

- [ ] **Step 3: Spawn background flush at server startup**

In `src/bin/aleph-server/commands/start/mod.rs`, locate where the embedding manager is constructed; add:

```rust
embedding_mgr.clone().spawn_background_flush();
```

- [ ] **Step 4: Add an integration test for retrieval before/after flush**

Append to `tests/note_layer_index_refresh.rs`:

```rust
#[tokio::test]
async fn retrieval_before_flush_returns_fts_only_after_flush_returns_hybrid() {
    // 1. Build ingestor + embedding_mgr with pending queue.
    // 2. Ingest a note; do NOT flush yet.
    // 3. Run hybrid_search_notes — should return the note via FTS.
    // 4. Flush; run again — should return the note via vector branch (score differs).
    // (Pseudo: implement against existing test fixtures.)
}
```

(Fill in the body using whatever test factories the retrieval module exposes.)

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::embedding_manager memory::notes
cargo test -p alephcore --test note_layer_index_refresh
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/apply.rs src/memory/dreaming/mod.rs src/bin/aleph-server/commands/start/mod.rs tests/note_layer_index_refresh.rs
git commit -m "feat(embedding): flush triggers on ingest tail, dream stage tails, 60s background"
```

---

## Task 10 (Phase B verification gate)

**Files:** none (verification only)

- [ ] **Step 1: Re-run write-amp bench**

```bash
cargo bench -p alephcore --bench notes_index_writes -- --nocapture
```
Expected: ≥70% reduction vs the pre-Task 2 baseline recorded in Task 1 Step 3.

- [ ] **Step 2: Re-run rebuild bench**

```bash
cargo bench -p alephcore --bench full_rebuild_1000_notes -- --nocapture
```
Expected: ≥4× speedup over a single-task baseline (run with `JoinSet` capacity forced to 1 to capture serial-equivalent number).

- [ ] **Step 3: EXPLAIN QUERY PLAN check**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema::tests::find_by_filename_uses_composite_index
```
Expected: green.

- [ ] **Step 4: Integration tests**

```bash
cargo test -p alephcore --test note_layer_index_refresh
```
Expected: green.

- [ ] **Step 5: A-bucket regression check**

```bash
cargo test -p alephcore --lib memory::notes
```
Expected: green (Phase A's tests still pass).

- [ ] **Step 6: Tag the phase**

```bash
git tag note-layer-phase-b-complete
```

Phase B done. Phase C2 (governance) builds on the gate-able write paths in `apply.rs` and `feedback_distill.rs`.
