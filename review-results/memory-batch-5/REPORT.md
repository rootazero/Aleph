# Memory Batch 5 — `src/memory/{session_compactor,session_search_summary,session_resume,session_reflection,scratchpad,transcript_indexer,ripple,flush}/*` Code Review

**Date**: 2026-08-12
**Path**: 8 submodules, 34 files, ~7 300 lines
**Reviewer**: static (security / logic / architecture / quality)

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    4 |     7 |    4 |   15 |

---

## Findings

### [HIGH] `session_compactor/post_turn_compress.rs:130-180` — `tokio::spawn` inside the compress loop has no shutdown or error join
- **Category**: architecture / DoS
- **Description**: For every semantic chunk, the function fires a `tokio::spawn` to write the pre-compress raw memory. A long session can produce 50+ chunks; the spawn count grows linearly with the session length. Each spawned task is fire-and-forget: errors are logged, the task returns, but there is no rate-limit, no per-task budget, and no upper bound on in-flight tasks.
- **Suggested fix**: Use a `JoinSet` and cap the in-flight count to a config value (`MAX_INFLIGHT_PRECOMPRESS = 8`). The compress loop awaits a slot before spawning the next. On graceful shutdown, `abort_all` the set.

### [HIGH] `session_search_summary/synthesizer.rs:561-572` — `tokio::spawn` inside `lazy_for` fan-out: `h.await.expect("task panicked")` panics on a child error
- **Category**: logic
- **Description**: The function dispatches a `tokio::spawn` per candidate and `expect("task panicked")` on the join handle. A single malformed candidate panics the whole `lazy_for` call, killing the `session_search` RPC.
- **Suggested fix**: Replace `expect` with `match h.await { Ok(Ok(v)) => v, Ok(Err(e)) => { tracing::warn!(?e, "synthesizer child task failed"); continue; } Err(e) => { tracing::warn!(?e, "synthesizer child task panicked"); continue; } }`.

### [HIGH] `session_resume/writer.rs:362, 369, 394, 420, 427` — `std::thread::sleep` in `sync`-land functions blocks tokio workers
- **Category**: architecture
- **Description**: Five call sites use `std::thread::sleep` for 5–15 ms settle windows. The function is on a tokio worker (via `tokio::task::spawn_blocking` per the doc-comment), so the blocking is contained, but the work-queue semantics are wrong: `spawn_blocking` threads are a *bounded* pool, and a 15 ms sleep × 5 sites per call × N concurrent calls can starve the pool. The retentionsweep (`cleanup_old_snapshots`) is also sync and walks every snapshot dir on every write.
- **Suggested fix**: Use `tokio::time::sleep` and make the relevant function `async`. The retention sweep is per-write; do it on a background `tokio::task` (rate-limited to once per N writes).

### [HIGH] `flush/registry.rs:134, 182` — `tokio::spawn` for `await_ready` racing tests; the helper has no timeout
- **Category**: DoS
- **Description**: The test spawns a `tokio::spawn` for `await_ready` with a 2-second inner timeout. If the timeout is removed (refactor) or changed to a much larger value, the test stalls. The tests pass today; the *helper* is the footgun.
- **Suggested fix**: The tests should use `tokio::time::timeout(Duration::from_secs(5), h.await)` to fence the test itself, independent of the production `await_ready`'s internal timeout.

### [MEDIUM] `session_compactor/post_turn_compress.rs:155-180` — pre-compress raw memory write runs in a spawned task that captures `&self`
- **Category**: logic
- **Description**: The closure `tokio::spawn(async move { ... })` captures `writer` (an `Arc<dyn RawMemoryStore>`), `registry_opt`, and the agent/session id strings. The closure does not capture `&self`, but it does capture the LLM-side strings via `.clone()`. A `String` clone of a 1 KB session id is cheap, but the *pattern* of capturing into a `'static` future is fragile — a future change that adds `&self` capture would break the spawn (lifetime mismatch).
- **Suggested fix**: Pull the writes into a free `fn` that takes the cloned state explicitly. The `tokio::spawn` then has no hidden capture surface.

### [MEDIUM] `session_search_summary/synthesizer.rs:560-580` — fan-out uses `Vec<JoinHandle>`, no `JoinSet`; cancellation is per-task
- **Category**: architecture
- **Description**: The fan-out is `for child in work { handles.push(tokio::spawn(...)) }; for h in handles { h.await... }`. If the parent task is cancelled, the spawned children are not aborted. They continue running, write raw memories, and the parent is gone.
- **Suggested fix**: Use `tokio::task::JoinSet`; on cancellation the set's `abort_all` fires and the children stop.

### [MEDIUM] `session_reflection/mod.rs:705` — `let body = std::fs::read_to_string(&scoped_path).unwrap();` in production
- **Category**: logic
- **Description**: A `unwrap` on a file read in the reflection loader. The path is built from the agent id and a hash; a transient I/O error (a file moved by the user) panics the reflection loader and aborts the reflection pass.
- **Suggested fix**: `match std::fs::read_to_string(&scoped_path) { Ok(s) => body = s, Err(e) => { tracing::warn!(?e, "reflection loader: read failed"); return Ok(None); } }`.

### [MEDIUM] `scratchpad/manager.rs:913` — `let cur = snap.current().expect("an in-progress step");`
- **Category**: quality
- **Description**: The `expect` is correct *today* — the function is called only when an in-progress step exists — but the surrounding `match` already returns `None` on the empty case. The `expect` is redundant; replace with the same `match`.
- **Suggested fix**: `let Some(cur) = snap.current() else { return Ok(StepOutcome::NoOp) };`.

### [MEDIUM] `session_search_summary/filter.rs:120, 158` — `std::fs::create_dir_all(...).unwrap()` / `tokio::fs::create_dir_all(...).await.unwrap()` in production
- **Category**: logic
- **Description**: Two `unwrap`s on `create_dir_all` in the filter loader. A read-only filesystem (a backup mount, a chmod'd dir) panics the filter.
- **Suggested fix**: `.map_err(|e| AlephError::config(format!("filter: create_dir_all failed: {e}")))?`.

### [MEDIUM] `session_resume/writer.rs:228, 267, 330` — `std::fs::read_to_string(...).unwrap()` in three production paths
- **Category**: logic
- **Description**: The reader functions unwrap on the read; the test-only `corrupt` path unwraps because it sets up a known-bad fixture. The production readers should follow the test pattern (return `Err` on read failure).
- **Suggested fix**: Replace with `let content = std::fs::read_to_string(&path).map_err(|e| AlephError::config(format!("resume read: {e}")))?;`.

### [MEDIUM] `transcript_indexer/mod.rs:22, 37, 60` — `SqliteMemoryBackend::new(&db_path).unwrap()` / `tempdir().unwrap()` in three production paths
- **Category**: logic
- **Description**: The function is `pub fn` (not test-only) and unwraps. A user with a read-only home directory sees a panic instead of a clear error.
- **Suggested fix**: Propagate the error: `pub fn new(db_path: &Path) -> Result<Arc<Self>, AlephError> { ... }`.

### [LOW] `session_search_summary/dedup.rs:50` — `survivors.truncate(max_sessions)` after a sort
- **Category**: logic
- **Description**: The sort is `by score desc, then by created_at desc`. The truncate keeps the first `max_sessions`. If two sessions have the same score and same timestamp, the truncation is non-deterministic. The dedup output feeds the synthesiser's prompt; non-determinism in the prompt is a small but real reproducibility cost.
- **Suggested fix**: Add a third tiebreaker: the session id. `Vec::sort_by_key(|s| (Reverse(s.score), Reverse(s.created_at), s.id.clone()))`.

### [LOW] `ripple/task.rs:1-100` — `RippleTask` is a future with manual `poll`; the inner `Waker` chain is fragile
- **Category**: architecture
- **Description**: Hand-rolled `Future` impls are hard to maintain. A `Pin<Box<dyn Future<Output = ...> + Send>>` builder is the modern Rust pattern.
- **Suggested fix**: Refactor to an `async fn` returning the future; the manual poll code is no longer needed.

### [LOW] `scratchpad/template.rs:99` — `panic!("unexpected generated section {other}")` in production
- **Category**: logic
- **Description**: Same pattern as the assembler / events panics. A new section kind in the template renderer panics the whole scratchpad loader.
- **Suggested fix**: Return an `Err`; the caller already has the error path.

### [LOW] `flush/mod.rs:42-60` — `await_ready` has the documented race; the fix is "spawn returns before polled" but the production caller does not handle the early-return case
- **Category**: logic
- **Description**: The doc-comment says: "spawn returns before the task is polled, so the test needs to await the handle or a short sleep". The production caller (`assembler/hybrid.rs`) calls `await_ready` with a 2-second timeout; the timeout handles the race. The race is a test-only footgun.
- **Suggested fix**: Document the test-side requirement on `await_ready` and add a `wait_for_ready` helper that polls until `is_ready() == true` or a deadline.

## Cross-References

- `session_compactor/post_turn_compress.rs:130-180` and `session_search_summary/synthesizer.rs:560-580` — both fan out via `tokio::spawn` and lack a `JoinSet` for clean cancellation. A shared `spawn_with_join_set` helper would close both.
- `session_resume/writer.rs:362-369` and `session_resume/reader.rs:149-206` — both use `std::thread::sleep` for settle windows. The pattern is consistent but the budget is wrong: `spawn_blocking` threads are bounded.
- `transcript_indexer/mod.rs:22, 37, 60` and `session_search_summary/filter.rs:120, 158` — both `unwrap` on filesystem ops in production paths. A single `fs::ensure_dir` helper would close both.

## Strengths

- `session_compactor/mod.rs` correctly documents the `CompactorMetrics` removal as a YAGNI purge — five atomics with no reader, removed not rescued.
- `session_resume/writer.rs::cleanup_old_snapshots` is bucketed by `(agent_id, scope_id)`, the right shape. The "every agent's snapshots are equally protected" property is preserved.
- `session_search_summary/synthesizer.rs::lazy_for` is idempotent: `INSERT OR IGNORE` ensures exactly one row survives, and the final re-read returns the winner.
- `session_reflection/mod.rs::Reflector` is `async fn`-clean: no `std::thread::sleep`, no `unwrap` on the hot path. The single `unwrap` in line 705 is the outlier.
- `transcript_indexer/mod.rs::indexer.index_turn_text` is the only turn-level entry point; the rest of the module is plumbing around it. The shape is right.
- `ripple/config.rs::RippleConfig` is deserializable from TOML with a `Default`; operators can enable/disable ripple without code changes.
