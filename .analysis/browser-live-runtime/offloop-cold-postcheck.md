# Off-loop cold path post-check (skill / hub cache / extension offload)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
Mode: read-only verification. No source edits, no formatting, no Gateway restart, no real-home/keyring/env mutation, dirty tree (185 entries) preserved. Only this report plus the cargo/rustfmt logs listed below were written.

## Environment gate

- Before each Cargo command: `tasklist` showed no cargo/rustc/clippy; free physical memory 12.8 / 13.6 / 13.5 GB (>= 4 GB ALLOW).
- Exported env exactly: `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`. All runs `--offline`, `--test-threads=1`, strictly sequential.

## Source inspected

- `src/hub/cache.rs:183` `async fn with_conn<T, F>(&self, f: F) -> rusqlite::Result<T>`: `lock_owned()` then `spawn_blocking`; panic -> `resume_unwind`; cancellation -> `SqliteFailure(SQLITE_INTERRUPT, "catalog blocking task cancelled")`.
- `src/skill/mod.rs:135` `merge_published_roots`, `:163` `merge_published_roots_off_loop`; `run_blocking_fs` helper; `src/skill/shared.rs` `get_or_try_init` (not latched on cancel).
- `src/extension/lifecycle.rs` / `src/extension/mod.rs`: `scan_plugins_blocking` via `crate::skill::run_blocking_fs("ExtensionManager::scan_plugins_blocking")`, `mount_inner` manifest parse via `run_blocking_fs("ExtensionManager::mount_inner.parse")`, `ScannedDir`/`ScanPlan`, legacy marker scanned in the blocking pass.
- `src/builtin_tools/skill_reader/read.rs`: `with_consent` gated `cfg(all(test, unix))`.

## Test-path safety audit (before any Cargo run)

- `skill::tests::{merge_published_roots*, run_blocking_fs*, the_cold_skill_path_routes_sync_fs_through_the_blocking_pool}`: injected equivalence closures, thread-id comparison, `include_str!` source guard. No filesystem, no home.
- `hub::cache::tests::*`: all `open_in_memory`. No disk SQLite.
- `extension::lifecycle::tests::{load_all_*, mount_*, reload_plugin_*}`: `IsolatedAlephHome` (ALEPH_HOME -> TempDir, in-process guard) + `tempfile::tempdir()`, `scan_claude_dirs=false`, `scan_project_dirs=false`. `DiscoveryManager::discover_plugins_with_extra` (`src/discovery/scanner.rs:286`) scans only `aleph_home/plugins` (resolved at construction under the isolated home), explicit extra parents, and `claude_home` only when set.
- `extension::claude_cache_tests::a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home`: `claude_home_override` TempDir.
- `gateway::lane` + `gateway::server::connection/`: grep for `home_dir|USERPROFILE|remove_dir|keyring|set_var|Config::load|get_config_dir|rusqlite|Connection::open` -> no hits.
- No `remove_dir_all` outside owned TempDirs; no keyring; no real-home path reached.

## Commands and results

| # | Command | rc | Wall | Result | Log |
|---|---|---|---|---|---|
| 1 | `cargo test -p alephcore --offline --lib -- --test-threads=1 skill::tests::merge_published_roots skill::tests::the_cold_skill_path_routes_sync_fs_through_the_blocking_pool skill::tests::run_blocking_fs hub::cache::tests:: extension::lifecycle::tests::load_all_ extension::lifecycle::tests::mount_ extension::lifecycle::tests::reload_plugin_is_unmount_then_mount extension::claude_cache_tests::a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home` | 0 | 833 s | 31 passed, 0 failed, 0 ignored, 20767 filtered | `offloop-cold-1-targeted.log` |
| 2 | `cargo test -p alephcore --offline --lib gateway::server::connection:: -- --test-threads=1` | 0 | 6 s | 28 passed, 0 failed, 20770 filtered | `offloop-cold-gatewayserverconnection.log` |
| 3 | `cargo test -p alephcore --offline --lib gateway::lane:: -- --test-threads=1` | 0 | 6 s | 21 passed, 0 failed, 20777 filtered | `offloop-cold-gatewaylane.log` |
| 4 | `cargo clippy -p alephcore --lib --no-deps --offline -- -D warnings` | 0 | 248 s | 0 warning/error lines | `offloop-cold-clippy.log` |
| 5 | `rustfmt --edition 2021 --check <105 changed/untracked src/**/*.rs>` | 1 | - | 17 files with diffs (see below) | `offloop-cold-rustfmt.log` |

Run 1 breakdown (31):
- skill (7): `merge_published_roots_literal_fallback_keeps_plugin_roots`, `merge_published_roots_reraises_an_equiv_panic` (should_panic), `merge_published_roots_runs_equiv_off_the_runtime_thread`, `merge_published_roots_skips_the_hop_without_published_dirs`, `run_blocking_fs_reraises_a_panic_on_the_caller` (should_panic), `run_blocking_fs_runs_off_the_runtime_thread`, `the_cold_skill_path_routes_sync_fs_through_the_blocking_pool`.
- hub::cache (12): incl. new `with_conn_runs_sqlite_off_the_runtime_thread`, `with_conn_reraises_a_panic_and_releases_the_guard`, `async_api_round_trips_through_the_blocking_boundary`, `blocked_sqlite_work_neither_pins_the_runtime_nor_admits_a_second_writer`, plus 8 pre-existing query/upsert/clear/count tests.
- extension (12): 3 `load_all_*`, 7 `mount_*`, `reload_plugin_is_unmount_then_mount_and_reload_rebuilds_everything`, claude_cache disabled/no-write test.

## rustfmt --check (not formatted, per instructions)

Files touched by this offload change are clean when checked individually (rc=0): `src/skill/mod.rs`, `src/skill/shared.rs`, `src/hub/cache.rs`, `src/extension/lifecycle.rs`, `src/extension/mod.rs`, `src/builtin_tools/skill_reader/read.rs`, `src/gateway/lane.rs`, `src/gateway/server/connection/{off_loop.rs,mod.rs,dispatch.rs}`.

Files with formatting diffs (all are in the dirty set, not part of the cold-offload change):
`src/builtin_tools/browser_tools/companion/process.rs`, `src/builtin_tools/browser_tools/present.rs`, `src/builtin_tools/browser_tools/present_tests.rs`, `src/builtin_tools/mod.rs`, `src/builtin_tools/process_registry.rs`, `src/executor/builtin_registry/builder/constructor/mod.rs`, `src/executor/builtin_registry/definitions.rs`, `src/gateway/handlers/connect.rs`, `src/gateway/mcp_face/auth.rs`, `src/gateway/server/browser_live_runtime.rs`, `src/gateway/server/browser_viewer_route.rs`, `src/gateway/server/connection/auth.rs`, `src/sandbox/managed.rs`, `src/sandbox/platforms/windows/elevation.rs`, `src/sandbox/platforms/windows/job.rs`, `src/sandbox/policy.rs`, `src/sandbox/workspace/mod.rs`.
- `src/gateway/server/connection/auth.rs:821/833/848`: `final_verdict(...).2.as_deref()` chain wants splitting; these lines are inside the dirty-tree added hunk `@@ -164,0 +368,552 @@` (test code), i.e. introduced by uncommitted work.
- Whether the other diffs pre-date this tree (exist at HEAD) was not checked.

Verdict: formatting gate FAILS for the dirty tree as a whole (17 files); PASSES for the files of the skill/hub-cache/extension/gateway-offloop change.

## Residual D1 limits (what is NOT proven)

- Extension cold runtime is NOT fully proven off-loop. The extension tests above are functional/regression tests under an isolated home; none asserts the thread boundary of `discover_and_mount` / `scan_plugins_blocking` / `mount_inner.parse` (no deterministic thread-id test; would need an `aleph_home` injection seam). The only thread-boundary proof for this path is indirect: `run_blocking_fs_runs_off_the_runtime_thread` + the `include_str!` source guard for the skill path.
- Remaining synchronous sites still on the async path (from extension-cold-implementation.md): WASM `load_wasm_effect` / Extism `load_plugin` (A14), `provision_data_dir` (A9), vault/secret resolve (A10), `projection.rs` `discover_skill_dirs`/`is_dir` under lock (A12/A13), `after_transition` `user_hook_configs` (hooks.json + `ProjectStore::shared().list()`), legacy `.disabled` migrate write/remove, sync `discovered_plugin_ids` / watcher `sync_user_hooks`.
- hub cache: cancellation path (`SQLITE_INTERRUPT` mapping) has no dedicated test; on-disk SQLite contention not exercised (in-memory only).
- `skill_reader::read::with_consent` tests now compile only on unix; not executed on this Windows host.
- No full `cargo test -p alephcore --lib`, no Gateway live/browser QA, no cross-platform run, no other crates.
