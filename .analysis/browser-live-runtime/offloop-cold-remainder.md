# Off-loop cold remainder: skill singleton init, scan_roots dedup, CatalogCache SQLite

Status: implemented, NOT compiled or tested (no cargo this phase). `rustfmt --edition 2021 --check` on the three touched files: rc=0.

## Changed files
- `src/skill/shared.rs`
- `src/skill/mod.rs`
- `src/hub/cache.rs`

## 1. `ensure_shared_skill_system_initialized` (shared.rs)
- The singleton is now fetched (`shared_skill_system().clone()`) and `default_skill_dirs()` is probed INSIDE `run_blocking_fs("ensure_shared_skill_system_initialized", ..)`. First construction (`SkillsConfig::load` -> `skills.toml`) therefore runs on the blocking pool.
- `INIT_CELL.get_or_try_init`: a cancelled blocking task (`None`) returns `Err(())`, so the latch is NOT set, a warn is logged and the next call retries. A panic is re-raised by `run_blocking_fs`. No "empty success".
- Doc comment now scopes the covered work and states extension loading (`ExtensionManager::ensure_loaded`) is still pending, and that the public sync `shared_skill_system()` still builds inline for any first caller that bypasses ensure.

## 2. `SkillSystem::scan_roots` (mod.rs)
- Base dirs and published plugin dirs are cloned first (read guard dropped; no lock across the hop).
- New `merge_published_roots` (pure, first-match identity, published also deduped against earlier published) and `merge_published_roots_off_loop` (run via `run_blocking_fs("SkillSystem::scan_roots")`; empty published list skips the hop).
- Cancellation: the fallback merges the cloned inputs with literal `==` equivalence, so plugin roots are never dropped (worst case: a symlink/case alias is not collapsed for one scan). Equivalence-closure panic is re-raised.

## 3. `CatalogCache` (cache.rs)
- Private `with_conn(f)`: `Arc::clone(&conn).lock_owned().await`, then `spawn_blocking(move || f(&mut guard))`. Owned guard moves into the blocking task, so writers stay serialised on the one connection (no duplicate connection, no new dep/runtime).
- Used by `upsert_many`, `query`, `replace_source`, `count_source`, `record_origin`, `origins`, `forget_installed_origin`. Transactions begin and commit entirely inside one blocking closure (atomicity unchanged). Public `Result` signatures unchanged; `open`/`open_in_memory` stay sync.
- Failure modes: closure panic -> `resume_unwind` on the caller (guard released on unwind, tokio mutex doesn't poison, dropped `Transaction` rolls back); non-panic `JoinError` (cancelled) -> `rusqlite::Error::SqliteFailure(SQLITE_INTERRUPT, msg)`; never success/default.

## Tests added (all unexecuted)
mod.rs: `merge_published_roots_runs_equiv_off_the_runtime_thread`, `merge_published_roots_skips_the_hop_without_published_dirs`, `merge_published_roots_reraises_an_equiv_panic`, `merge_published_roots_literal_fallback_keeps_plugin_roots`; source guard `the_cold_skill_path_routes_sync_fs_through_the_blocking_pool` extended (scan_roots needle; ensure singleton/default_skill_dirs must appear only after the `run_blocking_fs` marker).
cache.rs (`#[tokio::test]` current-thread, `open_in_memory`): `with_conn_runs_sqlite_off_the_runtime_thread`, `async_api_round_trips_through_the_blocking_boundary`, `blocked_sqlite_work_neither_pins_the_runtime_nor_admits_a_second_writer` (std mpsc started/release handshake; a driver future on the same single thread proves the runtime keeps polling while SQLite work is parked, and a second op queues behind the guard; 30 s recv_timeout is only a regression safety net), `with_conn_reraises_a_panic_and_releases_the_guard`.
Fixtures: in-memory SQLite or pure closures. No real home, `Config::load`, keyring, env mutation, or file deletion.

## Remaining limitations
- Nothing compiled/run; types (`Send + 'static`, `E: Send`) and clippy unverified. Verify with `cargo test -p alephcore --lib hub::cache skill::tests` and library clippy next phase.
- Extension loading (`ExtensionManager::ensure_loaded`) still pending, separate boundary.
- Public sync `shared_skill_system()` still constructs inline for first callers outside ensure, e.g. `src/hub/reconcile.rs:86` `collect_installed` and `src/gateway/handlers/skills.rs:13` (outside allowed files).
- `reload_file` not broadened.
- Owned inputs are cloned (entries/filter/origin) per call: small cost for off-loop safety.
- A dropped caller future does not cancel an already-started blocking write; it still commits (by design, no half-applied transaction).
- `with_conn` uses the global blocking pool (default 512 threads); SQLite `busy_timeout` waits now occupy a blocking thread rather than a worker.
