# Task #56 — `reveal_browser` intermittent failure: root cause / 根因报告

Date / 日期: 2026-10-13. No Native window was opened; all evidence is from Gateway logs, a headless Playwright repro and unit tests.
未打开任何 Native 窗口；证据来自 Gateway 日志、无头 Playwright 复现与单元测试。

## 1. Verdict / 结论

**Not a Panel bug and not a random race. / 不是 Panel 缺陷，也不是随机竞态。**
The Gateway runs ONE `tokio::select!` loop per WebSocket connection (`src/gateway/server/connection/mod.rs`) and awaited every RPC **inline** in it. While an RPC is running, that connection forwards no events and reads no further requests (head-of-line blocking).
Gateway 每条 WS 连接只有一个 select 循环，并在其中**内联 await** 每个 RPC；RPC 执行期间该连接不转发任何事件、不读取后续请求（队头阻塞）。

The Panel calls `extensions.catalog` at startup. Its handler (`handle_catalog` → `hub::reconcile::collect_installed` → `skill::ensure_shared_skill_system_initialized`) performs a **once-per-process** skill-system init that takes ~40 s on this machine (scans `~/.claude/skills`, `~/.agents/skills`, skill-guard on each). So **the first Panel after a Gateway (re)start** freezes for ~40 s; the `browser.live.resource` frame for that connection waits in its buffer, the harness checks the tab ~1.5 s after the resource and fails.
Panel 启动时调用 `extensions.catalog`，其处理器触发**每进程一次**的 skill 系统初始化（本机约 40 s）。因此 **Gateway 重启后的第一个 Panel** 冻结约 40 s，`browser.live.resource` 帧滞留在缓冲中，harness 在资源出现约 1.5 s 后检查 tab，失败。

| Window | Gateway state | `extensions.catalog` duration (log) | reveal |
|---|---|---|---|
| 5 | first Panel after restart (PID 1740) | 21:41:59.6 → 21:42:41.86 (~42 s); conn closed 5 ms after it ended | fail |
| 6 | Gateway NOT restarted | 09:33:20.089 → .140 (50 ms) | pass |
| 7 | first Panel after restart (PID 2700) | 11:28:50.06 → 11:29:30.93 (~41 s); `Failed to send response to 127.0.0.1:59206 (os error 10054)` at its end | fail |

Window 5's earlier diagnosis ("browser.live subscription missing") was the same freeze: the `events.subscribe` calls were queued behind the catalog. In window 7 the subscriptions happened to be sent before the catalog, but the frame forward was blocked; `stream.*` / `team.*` subscriptions never appear in the window-7 log for `:59206`, exactly as predicted.
窗口 5 先前诊断的“订阅缺失”其实是同一冻结（订阅排在 catalog 之后）；窗口 7 订阅先于 catalog 到达，但帧转发被阻塞。

## 2. Reproduction without Native / 无 Native 复现

Script: `.analysis/browser-live-runtime/reveal-repro/repro.js` (Playwright chromium headless, fresh context per iteration, unpaired Panel, fixture WS triggers `agent.run OPEN_FIXTURE`, DOM probe identical to the native monitor: toggle → `workspace-tab-browser` → `browser-live-decision` / `pairing-notice`). Env knobs: `REPRO_BASE`, `REPRO_GATEWAY_PID`, `REPRO_NO_TOKEN`, `REPRO_RUNS`.

| Target | Result |
|---|---|
| Live Gateway PID 2700 (warm) | 20/20 pass (+2 earlier), `--cold` (Chromium engine killed first) 1/1 pass |
| Isolated Gateway B (same pre-fix exe sha `8407d21a…`, own `ALEPH_HOME` under `reveal-repro/gwb/`, port 18792), **first Panel after start** | **1/1 fail with the exact window-7 signature**: fixture got `browser.live.resource` rev 1 at +2.2 s, Panel `tab=false decision=false notice=false` (also 3 s later). Gateway B log: Panel subscriptions through `session.artifact` at 12:35:46.53, then nothing until 12:36:27.78 while `extensions.catalog` ran; Panel console `Failed to subscribe to stream.run_accepted: Request timed out` at +31 s. (A second cold start reproduced it again: catalog 12:39:27 → 12:40:09.) |
| Same Gateway B, now warm | 5/5 pass |

So failure = cold catalog, deterministically; warm = pass. Native-only differences (Tauri origin, credential bridge) are not involved.
结论：冷 catalog 必然失败，热态必然通过；与 Native 特有差异无关。

**After-fix live rate: not measured.** The fix is in the Gateway; the running Gateway (PID 2700) may not be restarted and `target/debug/aleph-server.exe` is locked by it, so no fixed binary could be run. Verified by unit tests only (as instructed).
**修复后的实机比例未测**：修复在 Gateway，PID 2700 不得重启且其 exe 被锁定，仅以单元测试验证。

## 3. Fix / 修复

New `src/gateway/server/connection/off_loop.rs`; wired in `src/gateway/server/connection/mod.rs` (~40 lines):

- `dispatches_off_loop(method)`: Query-lane (read-only, never idempotency-keyed) methods, **excluding** `connect` and `events.*`.
- `OffLoopRpcs` (connection-owned `JoinSet<String>`, cap `MAX_OFF_LOOP_RPCS = 8`): such RPCs run as tasks; a new select arm `off_loop_rpcs.next_response(), if !off_loop_rpcs.is_empty()` writes each response (JSON-RPC ids make out-of-order completion wire-legal). At the cap the future is handed back and awaited inline (old behavior). A panicking dispatch answers `INTERNAL_ERROR` for its id. Dropping the loop aborts in-flight tasks.
- Unchanged: `connect`, subscription ledger, every Mutate/Execute/System lane (ordering + idempotency guard), login wall, rate limit, viewer gate (all run before the branch), event visibility fences (verified user, epoch, exact resource ids). No Panel or `qa/native_ipc` change.

Trade-off / 取舍: read-only responses may now arrive in completion order rather than request order on one socket; mutations stay ordered. A request issued before a re-`connect` may finish after it with its original caller snapshot (viewer methods still revalidate after the lane permit).

## 4. Verification / 验证

- `cargo test -p alephcore --lib --offline off_loop`: **6 passed** (method classification incl. `extensions.catalog`; slow task does not hold back a fast one; cap hands future back; panic → INTERNAL_ERROR with id; drop aborts; source guard that `mod.rs` uses the set).
- `cargo test -p alephcore --lib --offline gateway::`: **4583 passed, 5 failed, 3 ignored**. The 5 failures are in files this change does not touch (other agents' in-progress work in this worktree): `run_loop::tests::project_skill_block_none_when_no_project_skills`, `continuation_lifecycle::…side_session_retirement` (flags `src/browser/live/lifecycle.rs`), `handlers::events::…cli_carve_out…` (frame.rs browser_live topic constants), `handlers::pty::…global_pty_manager_is_tagged` (`src/builtin_tools/terminal*`), `visibility::…ambient_owner_call_site…` (`src/gateway/caller_identity.rs:464`).
- `rustfmt --check src/gateway/server/connection/off_loop.rs`: clean.
- `cargo clippy -p alephcore --lib --offline --no-deps -- -D warnings`: see section 6 for status.

## 5. Next Native window / 下一次 Native 窗口

**A Gateway rebuild + restart IS required** before the next window for the fix to take effect. Until then any first Panel after a restart reproduces the failure. Interim workaround without code: warm the Gateway (one `extensions.catalog` call, e.g. load the Panel once) before the window — window 6 passed exactly this way.
**下次窗口前必须重建并重启 Gateway**。临时规避：窗口前先预热（打开一次 Panel 或调一次 `extensions.catalog`）。

## 6. Residual risks & NOT done / 残余风险与未做事项

- Not done: after-fix live repro (Gateway restart forbidden); Native window; desktop/shell changes; Panel console capture in the native QA trace (would require touching `desktop/shell` Rust — recommended: forward sanitized `events.subscribe … ok|error` and `browser-live` lines into the QA trace); `aleph-panel` tests (no Panel code changed).
- The ~40 s cold `extensions.catalog` itself is not fixed; with this change it only delays the catalog response, not the connection. Recommend moving skill-system init to startup/background or caching `collect_installed`.
- Other slow inline paths remain by design: `connect` and non-Query lanes (a slow mutating RPC still blocks its connection). Unpaired Panels have no seed path, so a frame published before subscription is still never replayed.
- Gateway B side effects: it read (copied, "legacy copy left in place") `C:\Users\zou\.aleph\data/tasks.db` during cron-store migration on first start, cloned the official plugin repo into `reveal-repro/gwb/home`, and launched its own Chromium; Gateway B and its children were stopped afterwards. PID 2700 and mock LLM 2664 were not touched. `gwb/config.toml` holds a dummy `api_key = "mock-not-a-secret"`.
- clippy for alephcore was blocked by another agent's uncompiling WIP in `src/sandbox/platforms/windows/{driver,wfp}.rs` at 12:3x.
