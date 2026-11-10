# Findings — GX3 (session_store + session_manager + busy_queue)

Files reviewed: 24 (17556 LOC)

Counts: P0= 0 P1= 3 P2= 3 P3= 1

---

### GX3-01 [P1] Blocking SQLite I/O held directly on async task in `get_or_create`, `reset_session`, `add_message_full`, and several other `async fn` methods on `SessionManager`

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

### GX3-03 [P1] `get_or_create` in `SessionManager` silently swallows transition from `Created`/`Idle` → `Active`, returning stale `state` in the existing-session branch when the transition UPDATE matches 0 rows

- **Location**: `src/gateway/session_manager/ops/crud.rs:90-107` (`SessionManager::get_or_create`)
- **Failure mode**: The code runs an `UPDATE ... WHERE state IN ('created', 'idle')` and then checks `if state_update.is_ok_and(|n| n == 0)`. When `n == 0` (state was neither `created` nor `idle`, e.g. already `active` or `running`), it falls through to a bare `UPDATE SET last_active_at` with `.ok()` (error swallowed) and returns the `meta` read at the start of the function without refreshing its `state` field. In the success branch (`n > 0`) it correctly sets `meta.state = Some(SessionState::Active)`. But in the `n == 0` branch the returned `meta.state` reflects the state as of the initial SELECT — potentially `SessionState::Running` or `SessionState::Error` — while the actual DB row now has an updated `last_active_at`. More importantly, the `Stopped` guard at line 85 checks `meta.state` read from the initial SELECT, which is correct; but a race between a `stop()` call and `get_or_create` could allow the state to be `Stopped` in the DB while the initial SELECT read `Active`, bypassing the guard.
- **Evidence**:
  ```rust
  // crud.rs:85-107
  if let Some(SessionState::Stopped) = meta.state {
      return Err(SessionManagerError::SessionStopped(meta.key.clone()));
  }
  let state_update = conn.execute(
      "UPDATE sessions SET last_active_at = ?, state = 'active' WHERE key = ? AND state IN ('created', 'idle')",
      ...
  );
  if state_update.is_ok_and(|n| n == 0) {
      conn.execute("UPDATE sessions SET last_active_at = ? WHERE key = ?", ...).ok();
  } else {
      meta.state = Some(SessionState::Active);
  }
  return Ok(meta);  // meta.state may be stale in the n==0 branch
  ```
  The `Stopped` check and the state UPDATE are not atomic — a `close_session` (which sets `state='stopped'`) executing between the SELECT and the UPDATE would allow a stopped session to be resumed.
- **Suggested fix**: Re-read the current state from DB after the UPDATE pair (or use a single `UPDATE ... RETURNING state` / re-SELECT) to ensure the returned `meta.state` is current, and move the `Stopped` guard to be checked after the UPDATE rather than relying solely on the initial SELECT. The minimal fix: after both UPDATE paths, re-read `state` from DB and set `meta.state` accordingly, and reject `Stopped`. The connection mutex is already held across this entire block, so no additional locking is needed.
- **Confidence**: 0.75

---

### GX3-04 [P2] `record_enqueued` in `durable.rs` writes blocking I/O (`std::fs`) on the arrival path, which is called synchronously before spawning the delivery task

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

### GX3-05 [P2] `ensure_sessions_columns` in `migration.rs` constructs an `ALTER TABLE` SQL string by direct string interpolation of column names and type definitions without the safety validation used in `run_migrations`

- **Location**: `src/gateway/session_store/migration.rs:425-432` (`ensure_sessions_columns`)
- **Failure mode**: `run_migrations` (in `session_manager/mod.rs:434-449`) validates table names, column names, and column definitions with `is_safe_identifier` / `is_safe_column_def` before interpolating them into SQL. `ensure_sessions_columns` in `migration.rs` does the same interpolation (`format!("ALTER TABLE sessions ADD COLUMN {col} {ty}")`) but calls `conn.execute(&sql, [])` with no equivalent validation. The values come from the hardcoded `needed` slice (line 408-424), so there is no current injection vector. However, the absence of validation means any future edit to the `needed` slice that introduces a special character would bypass the guard silently. This is a maintainability-level safety gap: the two functions implement the same DDL pattern with divergent safety postures.
- **Evidence**:
  ```rust
  // migration.rs:425-432
  for (col, ty) in needed {
      if !existing.contains(*col) {
          let sql = format!("ALTER TABLE sessions ADD COLUMN {col} {ty}");
          conn.execute(&sql, []).map_err(...)?;
      }
  }
  ```
  vs. `run_migrations` which calls `is_safe_identifier(table)` etc. before the same `format!`.
- **Suggested fix**: Apply the same `is_safe_identifier`/`is_safe_column_def` guards used in `run_migrations` to the loop in `ensure_sessions_columns`, returning an error on a rejected identifier. Since the values are compile-time constants today, this will always pass — but the guard enforces the invariant against future edits. No behavioral change for current inputs.
- **Confidence**: 0.80

---

### GX3-06 [P2] `reinject_survivors` in `durable.rs` increments `reinjected` before the spawned task completes, so the returned count reflects tasks launched, not tasks delivered

- **Location**: `src/gateway/busy_queue/durable.rs:681` (`reinject_survivors`)
- **Failure mode**: `reinjected += 1` is incremented when a ticket is successfully registered and the delivery task is spawned (`tokio::spawn(...)`). If `register_run_in_lane` returns `Some(ticket)` but the spawned task immediately encounters `DeliveryOutcome::Rejected` (which cannot happen here — a `Some` ticket means it was registered — but `Executed(Err(...))` is possible), the count still says "reinjected." More concretely, the count is described by its caller and log as "re-delivered messages" but actually means "re-queued messages whose delivery task was spawned." For observability, a task that spawns and then immediately times out or is purged is still counted as reinjected. This is a documentation/semantic gap rather than a safety flaw, but it contradicts the log line (`"busy-queue: re-delivered messages queued before a restart"`) and the returned value used by the caller to decide whether a second pass is needed.
- **Evidence**:
  ```rust
  // durable.rs:620-681
  let ticket = super::register_run_in_lane(...);
  let Some(ticket) = ticket else {
      tracing::warn!(...); continue;
  };
  // ...
  tokio::spawn(async move { ... });
  reinjected += 1;  // counted here, before any delivery attempt
  ```
- **Suggested fix**: The count semantics are documented as "how many messages this call re-queued" in the doc comment, which matches the current implementation (registration = re-queuing). The log message should say "re-queued" rather than "re-delivered." Alternatively, thread a shared `AtomicUsize` into the spawned task and count only on `Executed(Ok(()))` for "delivered" semantics. The minimal fix is a one-word log change: `"busy-queue: re-queued {reinjected} messages from before a restart"`. Invariants preserved: delivery ordering, retry semantics, lane registration.
- **Confidence**: 0.70

---

### GX3-07 [P3] `add_message_full` builds a dynamic SQL string by `push_str` with conditional clauses, creating a parameter-position dependency between the string builder and the `params` vec that the compiler cannot verify

- **Location**: `src/gateway/session_manager/ops/crud.rs:372-392` (`add_message_full`)
- **Failure mode**: The SQL string is built conditionally (`if valid_transition { sql.push_str(", state = 'running'") }`, `if derived_title.is_some() { sql.push_str(", derived_title = ?") }`), and the `params` vec is built in a separate block with matching conditionals. The two blocks must agree on the number and order of `?` placeholders. This is the exact "positional parameters with no compiler backing" shape that the `NewMessage` struct was introduced to fix for `add_message_full`'s own signature. A future edit that adds a new conditional column and updates the SQL but not the params (or vice versa) silently sends the wrong value to the wrong column at runtime (SQLite does not type-check parameter counts until execution).
- **Evidence**:
  ```rust
  // crud.rs:372-392
  let mut session_update_sql = String::from("UPDATE sessions SET last_active_at = ?, ...");
  if valid_transition { session_update_sql.push_str(", state = 'running'"); }
  if derived_title.is_some() { session_update_sql.push_str(", derived_title = ?"); }
  session_update_sql.push_str(" WHERE key = ?");
  let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &preview];
  if let Some(ref dt) = derived_title { params.push(dt); }
  params.push(&key_str);
  ```
- **Suggested fix**: Extract the dynamic UPDATE into a small struct or builder that pairs each `(column, value)` at construction time, similar to what `add_usage` does in `modify.rs` (lines 18-41). The named pairing makes a mismatch a type error rather than a runtime bug. No behavioral change; preserves the existing conditional-update semantics and the two-token invariant (`state = 'running'` has no bound parameter and is safe as a literal).
- **Confidence**: 0.72

---

## Clean files

- `src/gateway/busy_queue/config.rs` — no finding
- `src/gateway/busy_queue/deliver.rs` — no finding
- `src/gateway/busy_queue/spawn.rs` — no finding (finding GX3-04 is about the call site pattern, not a defect within spawn.rs itself beyond what's noted)
- `src/gateway/session_store/error.rs` — no finding
- `src/gateway/session_store/types.rs` — no finding
- `src/gateway/session_store/hook_transcripts.rs` — no finding
- `src/gateway/session_store/file_backend/meta.rs` — no finding
- `src/gateway/session_manager/ops/mod.rs` — no finding
- `src/gateway/session_manager/ops/emit.rs` — no finding
- `src/gateway/session_manager/ops/identity.rs` — finding GX3-02 noted above (cleanup_expired FTS gap); otherwise no additional findings
- `src/gateway/session_manager/ops/query.rs` — no finding
- `src/gateway/session_manager/mod.rs` — no finding
- `src/gateway/session_manager/memory_lifecycle_tests.rs` — no finding
- `src/gateway/session_store/file_backend/archive_diagnostics_tests.rs` — no finding (test/diagnostic file only)
- `src/gateway/session_store/sqlite_backend/mod.rs` — no finding
- `src/gateway/session_manager/ops/tests.rs` — no finding
- `src/gateway/session_manager/tests.rs` — no finding