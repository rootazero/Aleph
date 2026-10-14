# Off-loop RPC fix report (D1 / D2 / D3)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
No commit, reset, renormalize, Gateway restart, native window, admin probe or
process kill was performed. Unrelated dirty/untracked files were not touched.

## Changed files (this patch only; these files also carry earlier uncommitted work)

| File | Change |
|---|---|
| `src/skill/mod.rs` | D1: new `pub(crate) async fn run_blocking_fs<T,F>(ctx, f) -> Option<T>` (spawn_blocking; panic re-raised via `resume_unwind`; cancellation logged via `log_join_err` and returns `None`). `rescan_dirs` recursive scan loop (tripwire, `exists`, `guess_source`, `scan_directory`) now runs inside it (None -> keep old registry). `load_or_cached_config_value` runs `Config::load` + serialization inside it (Err/None -> cached value). `collect_usage_snapshot` sidecar loop runs inside it. `full_status` computes the usage index before taking registry/config read locks. 3 tests. |
| `src/skill/shared.rs` | D1: `default_skill_dirs()` (`exists()` probes) moved inside the `INIT_CELL` initializer and run via `run_blocking_fs`; doc updated. |
| `src/gateway/server/connection/off_loop.rs` | D2: `OFF_LOOP_SHUTDOWN_GRACE` (2 s); `shutdown()` -> `shutdown_within(grace) -> bool`: clear queue, `abort_all`, drain `join_next` under `tokio::time::timeout`; on timeout log + `detach_all` (aborted tasks cancel at next yield). Module doc updated for D2 and D3. New test `shutdown_is_bounded_when_a_task_is_stuck_in_sync_code`; existing shutdown test asserts `shutdown()` returns true. |
| `src/gateway/server/connection/mod.rs` | D2 comment at teardown; login-wall message now `auth::LOGIN_WALL_MESSAGE` (single source). Ordering `off_loop_rpcs.shutdown().await` before `cleanup::run_cleanup(` unchanged (source guard still holds). |
| `src/gateway/server/connection/auth.rs` | `pub const LOGIN_WALL_MESSAGE` (text unchanged). |
| `src/gateway/server/connection/dispatch.rs` | D3: in `dispatch_for_connection`, for `off_loop::dispatches_off_loop(method)` re-read live `CallerSnapshot::read`, re-apply canonical `auth::wall_admits`; on failure return `AUTH_REQUIRED` + `LOGIN_WALL_MESSAGE` with the request id and never dispatch; otherwise dispatch under the LIVE snapshot so `process_request`'s existing admin gate (`method_requires_admin && caller_is_member`) sees a demotion. No new policy. Viewer revalidation and ordinary RPCs unchanged. |
| `src/gateway/server/handler.rs` | D3 tests: probe handler on `extensions.installed`; operator control; demoted-to-member -> ADMIN_REQUIRED, 0 hits; demoted-to-guest -> AUTH_REQUIRED/LOGIN_WALL_MESSAGE, 0 hits; queued behind a held lane permit then demoted -> refused, 0 hits; method is admin and off-loop. |

## Design notes

- D1: Only owned `PathBuf`s / closures go to the blocking pool. Manager state,
  `OnceCell`, and `RwLock`s stay on the async side; no `block_on`, no second
  manager. Cancellation of the outer future leaves the blocking job to finish
  and its result is dropped (the old registry/cached config stays).
- D2: Abort only lands at an `.await`. With D1, the audited handlers' cold fs
  work is on the blocking pool so abort lands promptly; the grace bounds the
  residual (e.g. CatalogCache sync SQLite). Past the grace, cleanup runs and
  the detached, already-aborted task is cancelled at its next yield. It holds
  only `Arc` clones of server state; it cannot write to the socket (the
  response channel is the JoinSet, now detached). Guarantee is therefore
  "teardown waits <= 2 s", not "no task outlives the connection".
- D3: The live re-read happens after lane acquisition, immediately before
  handler dispatch (same placement as viewer revalidation).

## Test-path audit (before Cargo)

Selected tests: off_loop unit tests (in-memory JoinSet, source guards),
handler tests using `viewer_conns` (in-memory `ConnectionState` map,
remote 203.0.113.7), `HandlerRegistry::new` (registration only),
`MiddlewareChain`/`RateLimiter`/`LaneManager` (in-memory), skill
`run_blocking_fs` (thread id / panic) and a source guard (`include_str!`).
No home dir, `.aleph`, keyring, SQLite/vault, env mutation, or deletion.
`skill::tests::init_with_temp_dir` (Config::load under `IsolatedAlephHome`)
was deliberately NOT run.

## Commands and results

1. `rustfmt --edition 2021 --check` on the 7 changed files: my hunks clean
   after `rustfmt --edition 2021 src/gateway/server/handler.rs`. Remaining
   diffs: `src/gateway/server/connection/auth.rs` ~821/833/848/861
   (`final_verdict(...).2.as_deref()` in pre-existing uncommitted tests, not
   from this patch; left untouched).
2. `py -B qa/browser_live/check_memory.py` -> `available_bytes=14167179264 required_bytes=4294967296 ALLOW`.
3. Exported `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`, then
   `cargo test --offline --lib -- skill::tests::run_blocking_fs skill::tests::the_cold_skill_path gateway::server::connection::off_loop gateway::server::handler::tests::off_loop gateway::server::handler::tests::queued_ gateway::server::handler::tests::dispatch_for_connection`
   -> exit 0, `19 passed; 0 failed; 20771 filtered out`, 0 compiler warnings.

## Unresolved limitations / not done

- D1 residual sync work on runtime threads (not moved): `scan_roots`
  `paths::equivalent` (may canonicalize), `CatalogCache` sync rusqlite under a
  tokio Mutex (`src/hub/cache.rs`, small indexed queries),
  `ExtensionManager::ensure_loaded` discovery (pre-loaded at boot in
  `agent_init`; its discovery interleaves sync fs with async mounting, so it
  cannot be moved wholesale safely), `reload_file` parse. No live timing proof
  that cold `extensions.catalog` no longer pins a worker; evidence is code
  routing + source guard + thread-id unit test.
- D2: background work can outlive teardown by up to its next yield point
  after the 2 s grace (documented); not eliminated.
- D3: ordinary (non-off-loop) lane-queued admin RPCs still dispatch under the
  request-arrival snapshot (pre-existing behavior, intentionally unchanged;
  existing test asserts `revalidate_for_dispatch(.., "config.get", &stale) == Some(stale)`).
- Not run: full suite, clippy, the skill tests that call `Config::load`,
  extension/hub handler tests, native/admin/gateway QA. The earlier 52-test
  set was not re-run as a whole.
