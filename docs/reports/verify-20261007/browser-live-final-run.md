# Browser-live final run / 浏览器 live 最终验收 (2026-10-11)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
Scope: verification only; no product code changed. / 仅验证，未改产品代码。

## Steps and evidence / 步骤与证据
1. Pre-stop check / 停止前核对: 127.0.0.1:18790 listener = Win PID **10212**; ExecutablePath = `<worktree>\target\debug\aleph-server.exe`; CommandLine `--config <worktree>/.analysis/browser-live-runtime/config.toml start`. Only this PID stopped with `Stop-Process -Id 10212` (old `server.pid` was a Bash PID, not used). After stop: process gone, 0 listeners on 18790.
2. Memory gate / 内存门: `python`/`python3` on this machine are Windows Store stubs (exit 49, no output), so the gate was run with `py qa/browser_live/check_memory.py` -> `available_bytes=13581840384 required_bytes=4294967296 ALLOW` (exit 0).
3. Build / 重建: `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo build --offline -p alephcore --bin aleph-server` -> exit 0 (1m29s; 1 pre-existing dead_code warning). Log: `.analysis/browser-live-runtime/server-build-rerun.log`. exe mtime 18:47:19 is later than latest wasm build (`wasm-final.log` 18:38:19, exit 0), so rust-embed picked up the new dist.
4. Start / 启动: PowerShell `Start-Process` with `ALEPH_HOME=<worktree>/.analysis/browser-live-runtime/home` and `--config <worktree>/.analysis/browser-live-runtime/config.toml start`. Real Win PID **8876** saved to `.analysis/browser-live-runtime/server-win.pid`. Listener on 18790 owned by 8876, path = this worktree's `target/debug/aleph-server.exe`; `GET http://127.0.0.1:18790/` -> HTTP 200 (after ~18s). Logs: `server-final2.log`, `server-final2.err.log` (benign startup warnings: "Session migration failed: no such table: sessions", MemoryReflector not wired).
5. `run-live.sh` -> `.analysis/browser-live-runtime/run-live-final2.log`:
   - revoke-live: qa exit=0, driver exit=0, 1 passed
   - navigate-fence: qa exit=0, driver exit=0, 1 passed
6. `run-all.sh` -> `.analysis/browser-live-runtime/run-all-final2.log`:
   - stream, control-all-entries, standalone-same-target, grant-boundary, binding-switch, reap: each `exit=0`, 1 passed
   - managed-companion: **exit=1** "ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL must be a real managed-companion viewer URL" -> BLOCKED (no production producer of such a URL)
   - Both scripts' own overall exit was 0 and was NOT trusted; per-case qa exit codes, driver exit codes and Playwright pass lines were parsed individually.

## Result / 结果
- **8 / 9 cases passed** (6 in run-all + 2 in run-live); **1 BLOCKED**: managed-companion (exit 1, expected, no production producer).
- Scripts print "grant issued <grant_id>"; no nonce/ticket/token/full URL was output or recorded here. URL files and secret files were consumed only by the scripts/drivers, not read into context.

## Not done / 未做
- Native shell tests not run; managed-companion not made to pass (no producer exists).
- `C:/Users/zou/.aleph`, HOME/USERPROFILE, main branch untouched; no commit; no unknown-file cleanup; no other process stopped.
- Server (PID 8876) left running for further inspection; not stopped. / 服务器仍在运行。
