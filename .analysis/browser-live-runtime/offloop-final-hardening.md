# Off-loop final hardening (2026-10-13) — targeted, NOT full D1

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`. No reset /
normalization / commit / Gateway restart / Native / admin / home / destructive actions. `.analysis/` not
added or force-tracked. Pre-existing dirty/untracked files untouched apart from the edits below.

## 1. Changes

1. `src/skill/mod.rs` — rescan generation fence.
   - `Inner` gains `rescan_ticket: AtomicU64`, `rescan_published: AtomicU64` (via `crate::sync_primitives`).
   - New `fn publish_if_newer(published: &AtomicU64, ticket: u64) -> bool` (caller holds the `registry`
     write lock, which serialises publication).
   - `pub(crate) async fn rescan_dirs(&self) -> bool`: takes `ticket = fetch_add(1)+1` BEFORE `scan_roots`
     (root-set read); blocking scan unchanged (no async lock held across `spawn_blocking`); under the
     registry write lock, a stale ticket is dropped (returns `true`, no snapshot rebuild — the newer
     publisher rebuilds). Returns `false` only when the blocking scan was cancelled (registry untouched).
   - `init` keeps its `()` API: `let _ = self.rescan_dirs().await;` (cancellation already logged).
   - New pure test `skill::tests::rescan_generation_fence_rejects_stale_publication`.
2. `src/skill/shared.rs` — `ensure_shared_skill_system_initialized`: `if !system.rescan_dirs().await
   { return Err(()); }` so a cancelled rescan no longer latches `INIT_CELL`; doc "Cancellation" paragraph
   now covers both blocking hops. Public API unchanged (`async fn ...()`).
3. `src/gateway/server/connection/mod.rs:237-240` comment now describes bounded
   `OffLoopRpcs::shutdown` (drop queued, abort active, wait <= `OFF_LOOP_SHUTDOWN_GRACE`) before cleanup.
   `src/gateway/server/connection/off_loop.rs` module docs no longer claim all audited cold work is on
   the blocking pool; they list the residual sync steps from `docs/reference/BROWSER_LIVE_VIEWER.md`
   (WASM/Extism loading, mount data-dir/vault, projection discovery, user-hook refresh, legacy marker
   migration).
4. `docs/reference/BROWSER_LIVE_VIEWER.md`: bilingual note at top that every cited `.analysis/...` path
   (also in `docs/reports/verify-20261007/*`) is local, worktree-only, intentionally preserved but
   untracked evidence.

Line endings: all five files remain CRLF throughout (crlf count == line count).

## 2. Static checks

- `rustfmt --edition 2021 --check` on `src/skill/mod.rs`, `src/skill/shared.rs`,
  `src/gateway/server/connection/off_loop.rs`: clean. On `src/gateway/server/connection/mod.rs`: the only
  diff reported is in the pre-existing, untouched `src/gateway/server/connection/auth.rs:821` (not fixed,
  out of scope).
- `git -c core.autocrlf=true diff --check` on `src/skill` + `src/gateway/server/connection`: clean.
  `docs/reference/BROWSER_LIVE_VIEWER.md` (untracked; checked with `--no-index`): pre-existing trailing
  whitespace at line 171 and blank line at EOF — not introduced here, not fixed.

## 3. Side-effect audit (before Cargo)

- skill filters: `rescan_generation_fence_*` (pure AtomicU64), `run_blocking_fs_*` (thread id / panic, no
  fs), `the_cold_skill_path_*` (`include_str!` source guard). No `SkillSystem::new` / shared singleton /
  home / TempDir in these tests. `skill::shared::tests` (constructs real singleton -> `get_config_dir`)
  intentionally NOT run.
- grep of `connection/off_loop.rs`, `connection/mod.rs`, and `handler.rs:1165-1610` for
  `home_dir|USERPROFILE|remove_dir|keyring|set_var|Config::load|get_config_dir|rusqlite|Connection::open|TempDir|tempfile|shared_skill`: no hits.
- No cargo/rustc running before start; free memory 12,410,990,592 bytes.

## 4. Cargo run (one command)

Env: `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0
RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`.

```
cargo test -p alephcore --offline --lib -- --test-threads=1 skill::tests::rescan_generation_fence skill::tests::run_blocking_fs skill::tests::the_cold_skill_path gateway::server::connection::off_loop gateway::server::handler::tests::off_loop gateway::server::handler::tests::queued_ gateway::server::handler::tests::dispatch_for_connection
```

Result: exit 0; **20 passed, 0 failed, 20779 filtered out** (19 previous + new fence test); 0 `warning`
lines; build 13m34s, real 13m37s. Log: `.analysis/browser-live-runtime/offloop-final-hardening-test.log`.

## 5. Not done / limits

- Fence test exercises `publish_if_newer` only; no deterministic concurrent two-rescan integration test
  (would need a controllable slow scan). Fence orders registry publication by ticket, not by filesystem
  read time: a higher ticket whose blocking scan read the disk earlier still wins (benign; dir set is
  monotone).
- `reload_file` / other direct registry writers are not fenced against an in-flight older rescan (unchanged
  pre-existing behaviour; rescan re-reads disk).
- Not run: clippy, `gateway::server::connection::` full, `gateway::lane::`, census tests,
  `skill::shared::tests`, workspace tests, browser-live / Native / admin / real cold `extensions.catalog`.
- **Not full D1**: residual synchronous work (WASM/Extism loading, mount data-dir/vault, projection
  discovery, user-hook refresh, legacy marker migration) remains on runtime workers. Overall status
  remains PARTIAL / not production ready.
