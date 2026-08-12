# Memory Batch 3 — `src/memory/store/*` Code Review

**Date**: 2026-08-12
**Path**: `src/memory/store/*` (22 files, ~11 011 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    4 |     7 |    4 |   15 |

---

## Findings

### [HIGH] `store/sqlite/notes/store_impl.rs:1-50` — `lock_conn!` macro recovers from a poisoned mutex unconditionally, hiding real panics
- **Category**: logic / safety
- **Description**: The macro's `unwrap_or_else(|e| e.into_inner())` recovers from a poisoned mutex. This is the *poison-safe* pattern, and the doc-comment acknowledges the intent. However, `into_inner()` returns the guard, and the connection it holds may be in a half-committed state — SQLite's `Connection` does not auto-rollback on `Drop` if the inner `*mut sqlite3` was panicking inside an active statement. A panic mid-statement leaves the connection with an open statement; the next `prepare` may fail with `SQLITE_BUSY` or return corrupted rows.
- **Suggested fix**: After `into_inner()`, call `conn.cache_flush()` (or `conn.close().ok()` and reopen). The first is a `SoftHeapLimit(0)` reset; the second is a hard reset. Pick the cheaper one. Either way, the recovery should be **visible** — `tracing::warn!` on every recovery so an operator sees a poison event.

### [HIGH] `store/sqlite/notes/store_impl.rs:1130-1175` — `prune_orphan_vectors` runs 5 dimensions × N orphans DELETE round-trips in one transaction
- **Category**: DoS
- **Description**: Each orphan rowid is deleted from every `notes_vec_{dim}` table (5 dimensions) plus the `notes_vec_map` table. A 50 k-note vault can produce 50 k × 6 = 300 k `DELETE` calls inside one `tx`. The transaction holds the connection mutex the entire time, blocking all recall calls.
- **Suggested fix**: One `DELETE FROM notes_vec_{dim} WHERE rowid IN (r1, r2, ...)` per dimension. SQLite supports up to 32 766 bind values; cap the per-call batch at 5 000 for predictability. Commit every N rows.

### [HIGH] `store/sqlite/notes/store_impl.rs:1280-1330` — `relink_unresolved` walks the entire `notes_links` table on a per-agent basis
- **Category**: DoS
- **Description**: The function reads every `notes_links` row for the agent, parses the `to_raw` field, and re-runs the resolver. For a 100 k-link vault this is a 100 k-row read followed by a 100 k-call `links::resolve`. No batching.
- **Suggested fix**: `WHERE status = 'dangling'` — the unresolved rows are tagged; do not re-resolve the resolved ones. Index the column (`CREATE INDEX idx_notes_links_status ON notes_links(agent_id, status)`). For a 100 k-link vault with 1 % dangling, this is a 1 000-row scan, not 100 000.

### [HIGH] `store/sqlite/routing_experience.rs:85-180` — `record_routing_experience` writes a row, then a map row, then a vec row — three separate round-trips
- **Category**: logic
- **Description**: A new routing experience is three `INSERT`s in sequence (row → vec_map → vec0). A panic between step 1 and step 3 leaves an orphan vec_map row that the next `recall_routing_experience` will read but never resolve. The `prune_orphan_vectors` analogue for `routing_exp_vec_map` does not exist.
- **Suggested fix**: Wrap in a `conn.transaction()` and panic-rollback propagates from the dropped `Transaction`. Add a periodic `prune_orphan_routing_experiences` analogous to the notes version. Or use a single `INSERT INTO routing_experiences (...) RETURNING rowid` then reuse that rowid for the map and vec0 inserts.

### [MEDIUM] `store/sqlite/mod.rs:118-127` — `SqliteMemoryBackend::new` calls `create_dir_all` even when the path is a regular file
- **Category**: logic
- **Description**: `let resolved: PathBuf = if db_path.is_dir() { db_path.join("memory.db") } else { db_path.to_path_buf() }`. If `db_path` is `/var/aleph/memory.db` and the operator typo'd it to `/var/aleph/memory.dba` (a new path that does not exist), `is_dir()` returns false and we proceed. Then `create_dir_all(parent)` creates `/var/aleph/` and the SQLite open *fails* (because `/var/aleph/memory.dba` is not a file we can open). The error message is then "Failed to open memory database" — confusing.
- **Suggested fix**: Reject the case where `db_path.exists() && !db_path.is_file() && !db_path.is_dir()` with a clear `AlephError::config("db_path is neither a file nor a directory")`. Pure diagnostics.

### [MEDIUM] `store/sqlite/notes/store_impl.rs:1216-1230` — `format!("DELETE FROM {t}")` on every embed write
- **Category**: performance
- **Description**: Every `upsert_embedding` rebuilds the SQL string for each of the 5 dimension tables, even though the dimension never changes within one call. The cost is ~1 µs per call — small, but every read in the per-note path compounds.
- **Suggested fix**: Precompute the per-dimension DELETE statements at backend construction; cache the prepared statements on the `SqliteMemoryBackend` itself.

### [MEDIUM] `store/sqlite/notes/store_impl.rs:1432` — `format!("SELECT embedding FROM {table}")` with the per-dimension table name rebuilt per call
- **Category**: performance
- **Description**: Same shape as above. The `table` is known at the start of the function; rebuilding the string per call is wasted work.
- **Suggested fix**: Same — precompute and cache.

### [MEDIUM] `store/sqlite/raw_memories.rs:750-770` — `count_unprocessed` runs a full table scan
- **Category**: DoS
- **Description**: `SELECT COUNT(*) FROM raw_memories WHERE is_processed = 0 AND agent_id = ?`. The `is_processed = 0` predicate is unindexed; on a 1 M-row table the count takes seconds. A nightly dream cycle that polls this counter hits the cost every iteration.
- **Suggested fix**: `CREATE INDEX IF NOT EXISTS idx_raw_memories_unprocessed ON raw_memories(agent_id, is_processed) WHERE is_processed = 0`. The partial index is small and counts are O(1) on the index.

### [MEDIUM] `store/sqlite/routing_experience.rs:307` — `format!("DELETE FROM {table} WHERE rowid = ?1")` for each table on each neighbour
- **Category**: DoS
- **Description**: `prune_routing_experiences` deletes N rows from M tables. Same shape as `prune_orphan_vectors` but for routing experiences. The fix is identical.
- **Suggested fix**: Same — multi-row `DELETE` per dimension.

### [MEDIUM] `store/sqlite/sessions.rs:22-115` — `get_daily_insight` / similar read methods hold the connection mutex across the row mapping
- **Category**: architecture
- **Description**: The macro is `lock_conn!`; the row mapping then runs while the guard is alive. The mapping is pure (no awaits), so no deadlock — but a future change that adds an `await` in the mapper (e.g. an LLM call to format the daily insight for the panel) would silently introduce a cross-await lock.
- **Suggested fix**: Restructure as a free function that takes a `&Connection` parameter and a thin async wrapper that locks + dispatches.

### [MEDIUM] `store/sqlite/vec.rs:10-25` — `register_sqlite_vec` uses `sqlite3_auto_extension` with `std::mem::transmute` of a `unsafe extern "C"` pointer
- **Category**: safety
- **Description**: The transmute is the canonical pattern for FFI entrypoints. The `unsafe` block is correct. However, the `sqlite3_vec_init as *const ()` cast is an `as`-cast that loses provenance; modern Rust (1.81+) prefers `std::ffi::c_void` casts or `ptr::from_exposed_addr`. The current code works on every supported toolchain but raises a future-UB question.
- **Suggested fix**: Replace with `sqlite3_vec_init as unsafe extern "C" fn(...) -> i32 as *const ()`. The cast is then identity-equivalent and provenance-clean. Pure style; no behavior change.

### [LOW] `store/sqlite/recall_signals.rs:181` — `Box<dyn ToSql>` allocation per row in batched insert
- **Category**: performance
- **Description**: `id.clone() as Box<dyn ToSql>` is a heap allocation per row. For 10 k signals this is 10 k small allocations. A `Vec<&str>` borrowed from the input would skip the box entirely.
- **Suggested fix**: Build a `Vec<&dyn ToSql>` from borrowed references when the input is `&[String]`.

### [LOW] `store/sqlite/dream_kv.rs:60-110` — `get_kv` / `set_kv` use `Value::Text(...)` for the value column; binary payloads must JSON-encode first
- **Category**: architecture
- **Description**: The schema is `key TEXT, value TEXT`. A `Vec<u8>` or `serde_json::Value` caller has to serialize before calling; a `String` caller can pass through. The asymmetry is fine, but the public API does not document the JSON expectation.
- **Suggested fix**: Rename the function `set_json_kv` / `get_json_kv` or accept a `serde::Serialize` / `Deserialize` typed API.

### [LOW] `store/sqlite/notes/store_impl.rs:1-50` — `impl NoteStore for SqliteMemoryBackend` is one giant `impl` block; >2 100 lines
- **Category**: architecture
- **Description**: The doc-comment acknowledges "this is a single indivisible trait `impl` block (>1000 lines)". The reason ("a Rust trait impl cannot be split across files") is true but the methods are still separable: each method's body could be a free function `fn index_note_impl(conn: &Connection, ...) -> Result<...>` and the trait method just dispatches. Splitting aids review and enables the same logic to be tested without instantiating the backend.
- **Suggested fix**: As the doc says, mechanical split. Out of scope for this audit pass but worth a follow-up.

### [LOW] `store/raw_memory.rs:413-469` — `expect("delegation carries detail JSON")` etc. four times
- **Category**: quality
- **Description**: A malformed raw memory row that lacks the expected detail JSON panics the read path. A simple `if detail.is_none() { return Ok(None) }` would skip cleanly.
- **Suggested fix**: Replace `expect` with `let Some(detail) = detail else { return Ok(None) };` — the row is malformed but other rows in the same query are still useful.

## Cross-References

- `store/sqlite/notes/store_impl.rs:1130-1175` and `store/sqlite/routing_experience.rs:307` — both iterate per-orphan DELETE round-trips. One helper (`fn batch_delete_per_dim(conn, rowids: &[i64])`) would close both.
- `store/sqlite/mod.rs:118-127` and `store/sqlite/vec.rs:10-25` — the `new` function's filesystem contract is silent about the case where the path is a typo of an existing file. Pair with the `vec::register_sqlite_vec` safety check: both should error clearly on a misconfigured `db_path`.
- `store/sqlite/notes/store_impl.rs:1-50` and `store/sqlite/sessions.rs:22-115` — `lock_conn!` is reused; the same recovery semantics apply. The `Mutex` recovery is the right shape, but the panic-mid-statement case needs the cache-flush follow-up.

## Strengths

- `store/sqlite/vec.rs::EMBEDDING_DIM_TABLES` is a single source of truth: table creation, dimension lookup, and delete-path sweep all derive from this one constant. Adding a dimension is a one-line change.
- `store/sqlite/mod.rs::raw_where` is shared by `get_raw_memories_dashboard` and `count_raw_memories` so the count and the list cannot drift. The `escape_like` helper is a defensive measure against the "100%" search matching every row.
- `store/sqlite/notes/store_impl.rs` `index_note`'s links reconcile is well-shaped: it builds a `desired: HashMap<to_note, DesiredEdge>` from markdown, reads `existing: HashMap<to_note, ...>`, then DELETE-then-UPSERT the delta. The "no re-stamp" guard at the relation column is the right defence against NoteWeave's labels being wiped on re-index.
- `store/sqlite/recall_signals.rs` is the only signal source; both auto-recall and explicit reflection land here with a `UNIQUE(note_path, query_hash, day_bucket, channel)` constraint. The dedup is owned by the schema, not the caller.
- `store/sqlite/dream_kv.rs::DistillRejectRecord` carries the *full* context, not just the fingerprint. The next distill prompt can replay it as negative feedback.
