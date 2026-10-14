# Off-loop D1/D2/D3 post-fix verification (read-only)

Date: 2026-10-13 22:07-22:35 (host clock). Worktree:
`D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`.
No source edits, no formatting, no reset/renormalize/commit, no Gateway/Native/admin
process touched. Only this report and `postfix-*.log` files were written under
`.analysis/browser-live-runtime/`. Dirty tree preserved.

## 1. Current-file check (before Cargo)

Confirmed present in the working tree (matches `offloop-fix-report.md`):
- `src/skill/mod.rs:113` `pub(crate) async fn run_blocking_fs<T, F>(ctx: &'static str, f: F) -> Option<T>`
  (spawn_blocking; `is_panic` -> `resume_unwind`; else `log_join_err` -> `None`).
  Call sites: `load_or_cached_config_value` (:299), `collect_usage_snapshot` (:384), `rescan_dirs` (:682).
- `src/skill/shared.rs:57` `super::run_blocking_fs(` for `default_skill_dirs`.
- `src/gateway/server/connection/off_loop.rs` (untracked new file): `OFF_LOOP_SHUTDOWN_GRACE` = 2 s (:61),
  `shutdown()` -> `shutdown_within(grace) -> bool` (:204) = queue.clear, abort_all, timeout-drain, on timeout `detach_all`.
- `src/gateway/server/connection/dispatch.rs:~168-186`: for `off_loop::dispatches_off_loop(method)`, live
  `CallerSnapshot::read`, `auth::wall_admits`, else `AUTH_REQUIRED` + `LOGIN_WALL_MESSAGE`; dispatch under live snapshot.
- `src/gateway/server/connection/auth.rs:13` `pub const LOGIN_WALL_MESSAGE`; used at `connection/mod.rs:496`,
  `dispatch.rs:180`, `handler.rs:1538`.

## 2. Side-effect audit of selected tests

- skill: `run_blocking_fs_*` (thread id / panic, no fs), `the_cold_skill_path_*` (`include_str!` source guard).
  `init_with_temp_dir` (Config::load under IsolatedAlephHome) is NOT matched by the filters and was not run.
- off_loop tests: in-memory JoinSet + `include_str!` source guard.
- handler tests matched (`off_loop*`, `queued_*`, `dispatch_for_connection*`, lines 1165-1600): in-memory
  `viewer_conns` / probe `MiddlewareChain`. The two TempDir users in handler.rs (:899 cron test, :1717
  visibility store) are not matched by these filters.
- `gateway::server::connection::` (auth + off_loop), `gateway::lane::`: grep of `src/gateway/lane.rs`,
  `src/gateway/server/connection/`, `src/gateway/server/handler.rs` for `home_dir|USERPROFILE|remove_dir|keyring|set_var|Config::load|get_config_dir|rusqlite|Connection::open` -> no hits.
- Guards in `src/gateway/visibility.rs` (:624, :1310, :1346): `CALLER_USER.sync_scope` and read-only
  source scans (`read_to_string`).
- Conclusion: no real home, keyring, SQLite/vault, env mutation, or deletion reached. Proceeded.

## 3. Commands and results

Pre-check: `tasklist | grep -iE "cargo|rustc|clippy"` -> none. Every Cargo command ran alone, after
`py -B qa/browser_live/check_memory.py` returned ALLOW (available 13.77-14.86 GB, required 4294967296), with:

```
export CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'
```

| # | Command | Exit | Result | Log |
|---|---|---|---|---|
| 1 | `cargo test -p alephcore --offline --lib -- --test-threads=1 skill::tests::run_blocking_fs skill::tests::the_cold_skill_path gateway::server::connection::off_loop gateway::server::handler::tests::off_loop gateway::server::handler::tests::queued_ gateway::server::handler::tests::dispatch_for_connection` | 0 | 19 passed, 0 failed, 20771 filtered; 0 `warning` lines; build+run 13m37s | `postfix-1-targeted19.log` |
| 2 | `cargo test -p alephcore --offline --lib gateway::server::connection:: -- --test-threads=1` | 0 | 28 passed, 0 failed (was 27 pre-fix; +1 = `shutdown_is_bounded_when_a_task_is_stuck_in_sync_code`) | `postfix-2-connection.log` |
| 3 | `cargo test -p alephcore --offline --lib gateway::lane:: -- --test-threads=1` | 0 | 21 passed, 0 failed | `postfix-3-lane.log` |
| 4 | `cargo test -p alephcore --offline --lib every_ambient_owner_call_site_is_in_the_census -- --test-threads=1` | 0 | 1 passed | `postfix-4-census.log` |
| 5 | `cargo test -p alephcore --offline --lib the_census_names_no_file_that_stopped_using_it -- --test-threads=1` | 0 | 1 passed | `postfix-5-census2.log` |
| 6 | `cargo test -p alephcore --offline --lib run_principal_sans_caller -- --test-threads=1` | 0 | 1 passed | `postfix-6-sans-caller.log` |
| 7 | `cargo clippy -p alephcore --lib --no-deps --offline -- -D warnings` | 0 | 0 warning/error lines; alephcore checked; 12m23s | `postfix-7-clippy.log` |
| 8 | `rustfmt --edition 2021 --check <file>` per file (no formatting applied) | see below | | |

Gateway regression set (2-6): 28 + 21 + 1 + 1 + 1 = 52 passed, 0 failed (pre-fix run: 27+21+3 = 51 for
these same filters; the pre-fix continuation / cli_carve_out filters were not rerun, per scope).

rustfmt --check:
- exit 0: `src/skill/mod.rs`, `src/skill/shared.rs`, `src/gateway/server/connection/off_loop.rs`,
  `src/gateway/server/connection/dispatch.rs`, `src/gateway/server/handler.rs`.
- exit 1: `src/gateway/server/connection/auth.rs` - 4 diffs at :821/:833/:848/:861, all
  `final_verdict(...).2.as_deref()` line-wrapping in auth tests (not the D3 `LOGIN_WALL_MESSAGE` hunk at :13).
  `src/gateway/server/connection/mod.rs` also exits 1, but only because rustfmt follows its child
  module `auth.rs`; the reported diffs are the same four auth.rs locations, none in mod.rs itself.
  Left untouched as instructed. Attribution to pre-existing work rests on the implementer report;
  not independently proven via git (auth.rs carries a large uncommitted diff overall).

## 4. Limitations / not proven

- D1 is NOT fully proven. Evidence is code routing + source guard + thread-id unit test only. No real cold
  `extensions.catalog` / `skills.status` timing showing a runtime worker is no longer pinned. Residual sync
  work on runtime threads remains (per implementer): `scan_roots` `paths::equivalent` (may canonicalize),
  `CatalogCache` sync rusqlite under tokio Mutex (`src/hub/cache.rs`), `ExtensionManager::ensure_loaded`
  discovery, `reload_file` parse.
- D2: guarantee is "teardown waits <= 2 s grace", not "no task outlives the connection"; a detached
  stuck task runs until its next yield. Test proves the bound with a 100 ms grace only.
- D3: only off-loop methods re-read the live snapshot; ordinary lane-queued admin RPCs still use the
  arrival snapshot (pre-existing, unchanged, not re-examined here).
- clippy ran on `--lib` (non-test cfg) only; new test code was not clippy-checked (`--tests` not run).
- Not run: full alephcore suite, skill tests calling `Config::load`, extension/hub handler tests,
  continuation / cli_carve_out filters, browser::live, native/AppContainer/admin, Gateway live QA.
- No failures observed.
