# Browser live scope run / 浏览器 live scope 验证

**Status / 状态:** 8 live cases PASS; managed-companion BLOCKED (separate, not pass). Unit/clippy as below. / 8 个真机 case 通过；managed-companion 阻塞（单列）。

Worktree: D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256
Env: `ALEPH_HOME=<worktree>/.analysis/browser-live-runtime/home`, `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`, `--offline`. `py -3 qa/browser_live/check_memory.py` = ALLOW before every step (11.4–15.2 GB avail, gate 4294967296).
Static audit: no real home/keyring/.aleph access in selected core browser / viewer-route / TUI tests (tempdir, injected runners/revokers, `/nonexistent`). `recording::` (uses `dirs::home_dir`) excluded.

## Hashes (sha256 prefix) / 哈希
| File | Hash |
|---|---|
| src/browser/cdp_backend/mod.rs (final) | 5eead3caacda03e8… (earlier run: 41b42437c5f0871b…) |
| src/browser/cdp_backend/operation.rs | 48da494affff6734… (doc-comment-only edit, before run 1 snapshot) |
| interfaces/tui/src/tui/commands.rs | dc8dc4c28ef48970… |
| interfaces/tui/src/lib.rs | 892505968c274598… |
| target/debug/aleph-server.exe | built 2026-10-11 19:37:28 from mod.rs 5eead3ca… (hash unchanged after build) |

Delta between first and final mod.rs: only 2 test asserts (lines ~1825, ~1851): `out.err().expect(..)` → `let Err(err) = out else { panic!(..) };`. Product logic unchanged by that edit.

## Unit / lint
| # | Command | Result |
|---|---|---|
| 0 | `rustfmt --edition 2021 --check src/browser/cdp_backend/mod.rs` | exit 0 |
| 1 | `cargo test -p alephcore --lib browser::cdp_backend:: -- --skip browser::cdp_backend::recording::` | 158 passed / 0 failed (ran on pre-assert-edit mod.rs; 9 new scope tests pass). recording:: skipped (known Windows/Unix fixture failures) → NOT whole-suite green |
| 1b | `cargo test -p alephcore --lib recorded_mutation` (final mod.rs) | 3 passed / 0 failed (the 2 edited tests + same-target receipt) |
| 2 | `… --lib browser::live::` | 118 passed |
| 3 | `… --lib gateway::server::browser_viewer_route` | 7 passed |
| 4 | `cargo clippy -p alephcore --all-targets` (final) | exit 0; only the 5 baseline warnings (pty/runtime.rs:111 `attach`; project_context.rs:223 `super::*`; slash_command_body/tests.rs:8 `Duration`; hook_transcripts.rs:42 imports; skill_reader/read.rs:157 `with_consent`). First run had 2 extra `err_expect` warnings → fixed |
| 5 | `cargo test -p aleph-tui` | lib 507 / bin 8 passed, 0 warnings (503→507; 5 browser_launch_* tests pass) |
| 6 | `cargo clippy -p aleph-tui --all-targets` | exit 0, 0 warnings |
Suites 1–3,5 were not re-run after the 2-line assert edit (rerun only 1b + clippy). This was a targeted re-verification chosen by the main agent because product logic was unchanged and only test asserts were edited; it was **not** a user approval. / 该定向复验由 main 依“产品逻辑未变、仅改测试断言”自行选择，**不是**用户批准。

## Server / live
- Pre-check: port 18790 owner PID 8876, exe = worktree target/debug/aleph-server.exe, `--config` worktree/.analysis/browser-live-runtime/config.toml start → stopped only PID 8876 (port freed).
- `cargo build --offline -p alephcore --bin aleph-server`: ok, 4m03s, baseline warning only.
- Start via `scope-start.ps1` (Start-Process, ALEPH_HOME explicit): new WinPID **11076** (server-win.pid); listener owner 11076, exe/--config verified; `GET /` HTTP 200.
- `bash .analysis/browser-live-runtime/run-live.sh` and `run-all.sh` (script exit 0 ignored; per-case rc/driver used):

| Case | qa exit | driver | Result |
|---|---|---|---|
| revoke-live | 0 | 0 | PASS |
| navigate-fence | 0 | 0 | PASS |
| stream | 0 | – | PASS |
| control-all-entries | 0 | – | PASS |
| standalone-same-target | 0 | – | PASS |
| grant-boundary | 0 | – | PASS |
| binding-switch | 0 | – | PASS |
| reap | 0 | – | PASS |
| managed-companion | **1** | – | **BLOCKED**: `ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL must be a real managed-companion viewer URL` (no producer) |

Local diagnostic logs (not audited for secrets; do not share raw logs): `.analysis/browser-live-runtime/scope-*.log`, `run-live-scope.log`, `run-all-scope.log`, `qa-<case>-scope.log`, `driver-<case>-scope.log`, `qa-mc-scope.err`, `scope-build-server.log`, `scope-clippy-core2.log`, `scope-test-rerun.log`, `scope-src-hash-*.txt`. The logs do not print the public secret/issuer URL, but they have NOT been proven fully redacted. Note: scripts write fixed-name `qa-<case>.log`; copies suffixed `-scope`. Server left running (PID 11076).

## Not done / 未做
Native tests, TUI launcher real-machine, managed-companion producer, TTL, full suites, strict clippy (-D warnings, #49), re-running suites 1–3/5 after the assert edit.

## Review coverage limits / 覆盖限制
**覆盖限制与未证明项**：两处 review 修复（core `cdp_backend/mod.rs` task-local scope 收紧；TUI launch 失败后 grant cleanup）已按定向复验验收，**不等同**整分支批准。scope-run 的静态审计只覆盖所选 core browser / viewer-route / TUI 测试，`recording::` 被排除。**未真机证明**：Native 真实 IPC、TUI launcher 真机、session reconnect、TTL、subframe、Obscura。后续 Task #49 在 Windows scoped run 中验证 `recording::` 19/19 green，但 managed 路径无生产 producer；不声称用户批准、production ready、全 suite 绿或 strict 绿。

**Review coverage limits / not proven:** the two review fixes (core `cdp_backend/mod.rs` task-local scope tightening; TUI failed-launch grant cleanup) were accepted via targeted re-verification and are **not** a whole-branch approval. The scope-run static audit covered only the selected core browser / viewer-route / TUI tests and excluded `recording::`. Not proven on a real machine: Native real IPC, TUI launcher, session reconnect, TTL, subframe, Obscura. Later Task #49 verified Windows `recording::` 19/19 green in a scoped run, but not the whole workspace; the managed path has no production producer. No claim of user approval, production readiness, whole-suite green, or strict green.

**Deferred 风险（最终只读 review；静态，未证明为运行缺陷）**：覆盖扩展到 core 的 owner_registry/control/grant/viewer_backend production 等，以及 client 新增的共享 connection / protocol decode / model / keyring target switch；reviewed scope 内 Critical 0、confirmed Important 0（两个原 Important 已修），仍**不是**整分支批准。已被反驳、不得当作已确认 bug：resource 256 淘汰、Fetch overflow 无测试、Panel 墓碑复活猜测。静态 Minor：全局 owner 串行锁 + 输入 permit 锁序有 250ms 拒绝的可用性代价；Fetch feed 满与 register 替换竞态可能挂起（已有 race 测试，无真机 stress）；frame 格式共享质量。client Minor：workspace retain/auto_reveal 无生产调用方、window.open 无 noopener、grant fragment 未清除、AuthHello.validate_shape 允许空 grant_id（当前 session 的 exact grant comparison 会拒绝它）。生产政策（device revoke / 4096 grant）仍需用户裁定；未扩大实现范围。

**Deferred risks (final read-only review; static, not proven runtime defects):** coverage extended to core owner_registry/control/grant/viewer_backend production code and the new shared client connection / protocol decode / model / keyring target switch; Critical 0 and confirmed Important 0 within the reviewed scope (the two original Importants are fixed), still **not** a whole-branch approval. Refuted, not to be kept as confirmed bugs: resource-256 eviction, Fetch overflow having no test, Panel tombstone resurrection. Static Minor: global owner serial lock + input-permit lock order costs availability (250ms rejection); Fetch feed-full vs register-replace race may hang (existing race tests, no real-machine stress); frame-format sharing quality. Client Minor: workspace retain/auto_reveal have no production caller, window.open lacks noopener, grant fragment not cleared, AuthHello.validate_shape allows an empty grant_id (currently rejected by the session's exact grant comparison). Production policy (device revoke / 4096 grants) still needs a user decision; implementation scope not expanded.

