# Fix brief — GX3

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX3.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX3.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX3.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX3.md

## FIX THESE (3)

### GX3-01 [P1] Blocking SQLite I/O held directly on async task in `get_or_create`, `reset_session`, `add_message_full`, and several other `async fn` methods on `SessionManager`
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/session_manager/ops/crud.rs:60` (`SessionManager::get_or_create`), `src/gateway/session_manager/ops/crud.rs:263` (`add_message_full`), `src/gateway/session_manager/ops/modify.rs:78` (`compact_session`), and many other `async fn` methods throughout `ops/`
- **Failure mode**: Any call path that locks `self.conn` (a `std::sync::Mutex<rusqlite::Connection>`) and then executes a SQLite query runs a blocking I/O call inline on a Tokio worker thread. The lock is held for the duration of the query. On a session with many messages, `compact_session` or `get_history` can hold the lock for tens of milliseconds, stalling that worker thread and starving all other futures sharing it. The module-level doc comment acknowledges this pattern exists for `list_sessions` and `search_messages` (which use `query_blocking`), but the remaining ~20 `async fn` methods execute blocking SQLite directly. The `query_blocking` helper exists precisely to avoid this — it is just not used consistently.
- **Evidence**:
  ```rust
  // ops/crud.rs:263
  let mut conn = self
      .conn
      .lock()
      .map_err(...)?;
  // ...SQLite queries run synchronously on the Tokio worker...
  let tx = conn.transaction()...
  tx.execute("INSERT INTO messages ...", ...)?;
  ```
  ```rust
  // ops/mod.rs:53-67 (the fix that exists but isn't used everywhere)
  pub(super) async fn query_blocking<T, F>(&self, f: F) -> Result<T, SessionManagerError>
  where F: FnOnce(&rusqlite::Connection) -> ...) + Send + 'static {
      let conn = self.conn.clone();
      tokio::task::spawn_blocking(move || { ... f(&guard) })...
  }
  ```
  The area contract explicitly states: "no blocking I/O (std fs/net/sleep) inside `async fn` on a runtime thread."
- **Suggested fix**: Route the blocking SQLite calls in `get_or_create`, `add_message_full`, `compact_session`, `close_session`, `reset_session`, `delete_session`, and the `modify.rs` methods through `query_blocking` (or `spawn_blocking`) rather than locking `conn` inline. The `query_blocking` pattern already exists and is used correctly for `list_sessions`/`search_messages`; applying it consistently preserves the same observable semantics and error types. Note: `add_message_full` currently calls `self.compact_session(...).await` after releasing the lock — this ordering is preserved.
- **Confidence**: 0.92

---

### GX3-02 [P1] `cleanup_expired` in `SessionManager` (SQLite) deletes messages but not FTS entries, leaving deleted content searchable
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/session_manager/ops/identity.rs:224` (`SessionManager::cleanup_expired`)
- **Failure mode**: When ephemeral sessions are expired and deleted, the code issues `DELETE FROM messages WHERE session_key = ?` but does NOT first delete from `messages_fts`. The FTS5 table is a content table (`content=messages`) using external content mode; after the underlying `messages` rows are deleted, the FTS index still contains those rows' content. A subsequent `search_messages` call (which JOINs `messages_fts` back to `messages`) will fail to return results for those rowids (the JOIN drops them), but if the FTS table is ever rebuilt or optimized, orphaned FTS entries cause corruption. More concretely, with the current `content=messages` external-content FTS schema, a `DELETE FROM messages_fts WHERE rowid IN (...)` is required before the source rows are removed, matching what `reset_session` and `delete_session` do correctly.
- **Evidence**:
  ```rust
  // identity.rs:224-228
  for key in &keys {
      conn.execute("DELETE FROM messages WHERE session_key = ?", params![key])
          .ok();
      if conn.execute("DELETE FROM sessions WHERE key = ?", params![key]).is_ok() {
          deleted += 1;
      }
  }
  ```
  Compare to `reset_session` (crud.rs:657-662) which does the FTS cleanup first:
  ```rust
  conn.execute(
      "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)",
      params![&key_str],
  ).ok();
  ```
- **Suggested fix**: Add `conn.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)", params![key]).ok();` before `DELETE FROM messages` inside the loop in `cleanup_expired`, matching the pattern used in `reset_session` and `delete_session`. This preserves the "FTS search degrades gracefully" invariant already documented in the codebase.
- **Confidence**: 0.90

---

### GX3-04 [P2] `record_enqueued` in `durable.rs` writes blocking I/O (`std::fs`) on the arrival path, which is called synchronously before spawning the delivery task
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/busy_queue/durable.rs:285` (`record_enqueued`), called from `src/gateway/busy_queue/spawn.rs:72`
- **Failure mode**: `record_enqueued` calls `std::fs::create_dir_all`, `serde_json::to_vec_pretty`, and `crate::utils::atomic_io::write_atomic` (which involves a `std::fs::write` + `std::fs::rename`) synchronously. `spawn_queued_run` calls this on the async arrival path — `if ticket.is_some() && durable::is_armed() { durable::record_enqueued(...) }` — before spawning the delivery task. This blocks the Tokio worker thread for the duration of the filesystem write. On a slow disk this can be tens of milliseconds; on a high-throughput channel path this adds latency to every queued message's registration.
- **Evidence**:
  ```rust
  // spawn.rs:71-73
  if ticket.is_some() && durable::is_armed() {
      durable::record_enqueued(durable::QueuedRunPayload::from_request(&request, locale));
  }
  // tokio::spawn(async move { ... }) comes after
  ```
  ```rust
  // durable.rs:310
  if let Err(e) = write_entry(&root, &entry) { ... }
  // write_entry calls std::fs::create_dir_all + write_atomic (sync rename)
  ```
- **Suggested fix**: Move `record_enqueued` into the spawned task (before `deliver_with_ticket`) rather than on the synchronous arrival path. The ordering guarantee only requires the ticket to be taken synchronously; the durability write does not need to precede the spawn, only to precede the first delivery attempt. Alternatively, wrap the call in `tokio::task::spawn_blocking`. The ticket is already taken (line 62-67 of spawn.rs) before this call, so lane ordering is unaffected.
- **Confidence**: 0.82

---


## DO NOT FIX (4) — refuted / deferred by adjudication or routing

- GX3-03: ### GX3-03 [P1] `get_or_create` in `SessionManager` silently swallows transition from `Created`/`Idle` → `Active`, returning stale `state` in the existing-session branch when the transition UPDATE matches 0 rows
- GX3-05: ### GX3-05 [P2] `ensure_sessions_columns` in `migration.rs` constructs an `ALTER TABLE` SQL string by direct string interpolation of column names and type definitions without the safety validation used in `run_migrations`
- GX3-06: ### GX3-06 [P2] `reinject_survivors` in `durable.rs` increments `reinjected` before the spawned task completes, so the returned count reflects tasks launched, not tasks delivered
- GX3-07: ### GX3-07 [P3] `add_message_full` builds a dynamic SQL string by `push_str` with conditional clauses, creating a parameter-position dependency between the string builder and the `params` vec that the compiler cannot verify
