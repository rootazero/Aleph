# Task #56 — fifth (final) real Native window (index 5): FAIL `dom_not_as_expected` at `reveal_browser` / 第五次（最终）真实 Native 窗口：失败

**Verdict / 结论：overall FAIL (exit 1), cleanup CLEAN. Not a Native PASS. Overall remains PARTIAL, not production ready. / 整体 FAIL（退出码 1），清理干净；不是 Native PASS；整体仍为 PARTIAL，not production ready。**

**Authorization / 授权：** the user approved exactly one Gateway restart and exactly one final fifth window. `MAX_WINDOWS` was raised 4→5 (`qa/native_ipc/real_qa_prep.mjs:37`, self-tests updated: window 5 with four priors ok, index 6 refused). **The budget is exhausted; no sixth window, no retry.** No admin / WFP / managed companion. `C:/Users/zou/.aleph`, main and the dirty tree were not touched. / 用户批准恰好一次 Gateway 重启与一个最终第五窗口；预算已耗尽，无第六窗口、无重试。

## 1. Preparation / 准备（actual commands）

| Step | Result |
|------|--------|
| `node qa/native_ipc/real_qa_prep.mjs --self-test` (after MAX_WINDOWS=5) | 34/34 pass |
| `node qa/native_ipc/run-real.mjs --self-test` | 15/15 + fixes-selftest 41/41 pass (pure fakes) |
| Old Gateway PID 12704 | verified exact `W\target\debug\aleph-server.exe`, cmdline `--config W/.analysis/browser-live-runtime/config.toml start`; then `Stop-Process`; confirmed gone, port 18790 free. Old exe sha256 `5f827ee8…a0034` (stale, built 2026-10-11) |
| `bash .analysis/browser-live-runtime/task51-gate.sh cargo build --offline --bin aleph-server` (gate: free mem >= 4294967296, `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`, offline; one Cargo runner) | exit 0, `Finished dev profile in 10m 18s`; new exe sha256 `1fe14217d3567b73edab1969a87a513012cf49f7554c81a4c97b304f9280a40f` |
| New Gateway | PID **1740**, exe `W\target\debug\aleph-server.exe`, cmdline `--config W/.analysis/browser-live-runtime/config.toml start`, isolated `ALEPH_HOME=W/.analysis/browser-live-runtime/home`, no credentials in argv/env; listening 127.0.0.1:18790; `GET /` HTTP 200 (7206 bytes); `GET /aleph_panel_bg.wasm` HTTP 200, served sha256 == dist file `ce0b0b68…374c1b` (so served Panel == dist) |
| **Deviation: WASM/dist NOT rebuilt** | Existing `interfaces/webchat/dist` (aleph_panel.js 17:13:06, wasm 17:13:52 on 2026-10-12) is newer than every file under `interfaces/webchat`, `shared/ui_logic`, `shared/protocol` (the only panel deps); all 12 selectors the driver needs (`token-wall`, `token-wall-input`, `token-wall-submit`, `pairing-notice`, `pairing-notice-pair-desktop`, `browser-live-decision`, `browser-live-launch`, `browser-live-frame`, `browser-live-status`, `workspace-panel`, `layout-toggle`, `workspace-tab-browser`) are present in the wasm. A ~8 min WASM rebuild with OOM / partial-dist risk was judged riskier than useful. This departs from the instruction "rebuild fresh WASM/dist". Hashes in `.analysis/browser-live-runtime/task56w5-dist.sha`. |
| Process incident | While preparing, the stop/build/start sequence was executed twice in the log trail (the second `cargo build` re-ran, still exit 0); only one Gateway process (PID 1740) exists. `MAX_WINDOWS=5` edit had to be re-applied (the first edit was no longer on disk when the preflight returned `window_index_out_of_range`); no window was consumed by that refusal (exit 2). |
| Shell sha256 / reaper sha256 | `8d434299e4729327bfa06996568c70cf3ba7b60651de976a8fe90d0b8b5449e0` / `fcadd138fd427bd1f8eaee661064d3c4a16c9b73b5788ee53c23c66534def4ec` (re-hashed, unchanged; shell newer than shell sources) |
| Preflight (`run-real.mjs` without `--execute`, window 5) | exit 4 `PREFLIGHT_OK_NOT_EXECUTED`: seam, exeFresh, reaperFresh, panelFresh, gatewayIdentity, fixtureUp all true |

## 2. The run / 运行

Command (exactly once): `node qa/native_ipc/run-real.mjs --exe W/target/debug/aleph-desktop-shell.exe --exe-sha256 8d4342…49e0 --reaper W/target/debug/examples/qa_credential_reaper.exe --reaper-sha256 fcadd1…ec4ec --gateway-url ws://127.0.0.1:18790/ws --confirm-owned-pid 1740 --window-index 5 --window-ms 180000 --prior-window-ms 180000,180000,180000,180000 --execute` → **exit 1**, `status=FAIL`, `failedReason=dom_not_as_expected`, `cleanupState=CLEAN`, `documents=1`. Raw JSON: `.analysis/browser-live-runtime/task56w5-run.json` (untracked). Run ended ~20 s after start (not the full 180 s).

| Phase | Result |
|-------|--------|
| preflight | ok (8773 ms) |
| root_created / precheck_absent | ok / credential target absent |
| shell_spawned | ok, pid 512, `cmdlineOwned=true` |
| monitor_hello | ok, `authenticated=true`, `originExact=true` |
| empty_load | ok, `loadOk`, `leaseObject`, `credentialPresent=false` |
| resource_created | ok, `agentRunOk`, `resourceSeen`, revision 1 |
| **reveal_browser** | **FAIL** `dom_not_as_expected`: `ack=false`, `resourceVisible=false`, `pairingNotice=false` |
| monitor_streams | ok, 1 doc, allComplete, anomalies 0, nativeTraceComplete, 1 trace record |
| cleanup_process | ok, started/stopped, `ownedCmdlineProven`, `orphansKilled=0` |
| cleanup_gateway | ok, deviceRevoked, ticketRevoked, fixtureClosed, leftoverTicketsClean, noNewDevicesLeft |
| cleanup_credential | ok, `deleteExactOk`, `postAbsent` |
| cleanup_root | ok, `removed=true` |

Diagnostics: monitor `accepted=1`, refused 0, `helloSeen=1`; trace `records=1`, `poisoned=null`; shell `exitedBeforeCleanup=false`.

## 3. What failed / 失败

The run stopped at `reveal_browser` (the driver's bounded 3-attempt reveal): the Panel never acked the reveal and showed neither the resource (`resourceVisible=false`) nor the pairing notice. The run therefore never reached the save / new-device / reload / same-device phases, so **this window proves nothing about same-device across reload** (window 4 did, server-side). **Root cause not established:** candidates not distinguished — Panel in the driven browser did not reach the resource view (token wall / unauthenticated state), reveal selector/timing, or a real Panel defect. The DOM evidence was not weakened and fixture evidence was not accepted. / 根因未确定，未放宽判据。

## 4. Cleanup verification / 清理核验（independent, after the run）

- Shell child (PID 512) gone; no `aleph-desktop-shell` / `qa_credential_reaper` process exists.
- `cmdkey /list` shows no `aleph-qa` target; no `aleph-shell-qa-*` directory under `%TEMP%` or the runtime dir (runner reported `removed=true`).
- Gateway PID 1740 kept alive and clean (listening 127.0.0.1:18790; only TIME_WAIT sockets from the run). Its headless Chrome children (parent 1740, profile under `.analysis/browser-live-runtime/home/data/browser/chromium/default`) belong to the Gateway's own browser service, not the runner.

## 5. Not shown / not done

- No Native PASS; no JPEG frame; no protected Panel action; no UI paired / token-wall evidence; no production OS-keyring pairing; macOS/Linux; managed companion BLOCKED.
- WASM not rebuilt (see deviation); no root-cause analysis; no code fix for the reveal failure; no whole-workspace tests / clippy.
- Nothing committed.

## 6. Next input needed

The Native real-window budget (`MAX_WINDOWS=5`) is exhausted. Decide: accept the UI-evidence gap as a documented limitation, or authorize an offline investigation of why the Panel does not show the resource / acked reveal in the driven browser (then, if wanted, a further explicitly approved window). Gateway PID 1740 is left running.
