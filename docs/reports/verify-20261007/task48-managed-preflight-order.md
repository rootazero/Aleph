# #48 managed companion: fail-closed ordering at the sandbox seam

## Verdict / 结论
**EN.** #48 is NOT complete and `managed-companion` QA stays BLOCKED. No managed producer was run; no real browser, admin, WFP, AppContainer, keyring or Gateway action was taken. No non-Windows platform can launch a managed service from this Windows checkout without emulation: macOS `SeatbeltDriver::spawn_managed` needs a macOS host (never compiled/verified here), Linux bwrap/init has no `spawn_managed` (trait default = `UnsupportedPolicy`), Windows is blocked by the wrapper PID / no per-run WFP confinement / non-elevated token (see `windows-managed-blockers-run.md`).

**中文.** #48 **未完成**，`managed-companion` 真机 QA 仍 BLOCKED。未运行任何托管 producer，未触碰真实浏览器/管理员/WFP/AppContainer/keyring/Gateway。在本 Windows 检出中，没有任何非 Windows 平台可在不做平台模拟的前提下启动真实托管服务：macOS 需 macOS 主机（此处从未编译/验证），Linux bwrap/init 没有 `spawn_managed`（trait 默认 `UnsupportedPolicy`），Windows 被包装进程 PID / 无 per-run WFP 约束 / 非提权 token 阻塞。

## Finding / 发现
`WorkspaceSandbox::spawn_managed` (`src/sandbox/workspace/mod.rs`) ran `prepare_command` (hooks, approval card, proxy/DNS) BEFORE the driver had a chance to say "this host can never hand over a service child". Production `SandboxLauncher::launch` preflights first so it was not reachable there, but the seam itself did not enforce it: a direct caller with an elevated request on Windows/Linux would raise an approval card for a spawn that is certain to be refused.

## Change / 改动 (only `src/sandbox/workspace/mod.rs`)
- `spawn_managed`: call `self.os_driver.managed_spawn_preflight()?` right after the fork refusal and before `prepare_command`. Drivers that do support managed spawn (macOS) are unaffected (preflight is `Ok`).
- Tests: `managed_spawn_on_unsupported_driver_never_raises_an_approval` (elevated request, counting requester, expects `UnsupportedPolicy{feature:"managed_spawn"}` and 0 approvals); `#[cfg(windows)] windows_driver_managed_spawn_is_refused_before_approval` (real `WindowsSandboxDriver` through the real `WorkspaceSandbox`, strict and elevated caps, preflight and spawn both refuse with the typed blocker report naming the wrapper PID, 0 approvals).

## Verification (actual) / 实测
Pre-audit: filtered tests use `tempfile::tempdir()` roots, fake requesters, one read-only process-token query; no home/keyring/`.aleph`/`remove_dir_all` in the new tests.
Command: `bash .analysis/browser-live-runtime/task51-gate.sh cargo test -p alephcore --lib --offline sandbox::workspace::tests:: -- --test-threads=1 managed windows_driver` (memory gate >= 4 GiB, `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`, offline, single runner).
Log `.analysis/browser-live-runtime/task48-preflight-test.log`: exit 0, `71 passed; 0 failed; 20674 filtered out`, no warnings. Includes both new tests, the 4 pre-existing `sandbox::workspace::tests::managed_*`, `sandbox::platforms::windows::*` (blockers) and companion `*managed*` tests.

## Not done / 未做
- Did not run the new approval test against the pre-fix code (red check not performed); by inspection it fails without the preflight (FakeDriver + AllowAll network => approval requested).
- No clippy / fmt run; no macOS/Linux build; macOS seatbelt `(deny process-fork)` and PID identity remain unverified.
- No producer, no QA `managed-companion` run, no Panel/TUI changes, no commit.
- Windows blockers unchanged: needs an elevated disposable session for the WFP probe, a production suspended launcher, and a PID-reporting handoff.
