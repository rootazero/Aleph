# Task #56 — seventh real Native window (index 7, after the seed / `connected_since` fix): FAIL `dom_not_as_expected` at `reveal_browser` / 第七次真实 Native 窗口：失败

**Verdict / 结论：overall FAIL (exit 1), cleanup CLEAN. Not a Native PASS; overall remains PARTIAL, not production ready.**
**整体 FAIL（退出码 1），清理干净；不是 Native PASS；整体仍为 PARTIAL，not production ready。**

Regression vs window 6 / 相对第六窗口的退步：window 6 passed `reveal_browser` and failed later at `server_same_device`; window 7 failed **earlier**, at `reveal_browser` (the window-5 failure point), so pairing, save, reload and `server_same_device` never ran. **The new `connected_since` device-edge witness and the post-reconnect seed were NOT exercised by this window.** / 第七窗口在 `reveal_browser` 就失败（与第五窗口同一失败点），配对/保存/重载/`server_same_device` 均未执行；新的 `connected_since` 判据与重连后 seed **没有被这次窗口验证到**。

**Authorization / 授权：** user approved one Gateway rebuild + restart, one dist rebuild, and exactly ONE seventh window (fresh credential namespace, cleanup). `MAX_WINDOWS` 6→7. **The budget is used up: no eighth window, no retry.** `C:/Users/zou/.aleph` was only checked for existence. Nothing committed. / 预算已用完：不跑第八窗口、不重试。

## 1. Build and Gateway restart / 构建与 Gateway 重启

| Item | Result |
|------|--------|
| Cargo exclusivity | before each cargo run `tasklist` showed no cargo/rustc; `py -B qa/browser_live/check_memory.py` → ALLOW (`available_bytes=13590429696` before the server build, `14387392512` before `just wasm`; required 4294967296) |
| Old Gateway | PID **1740**, image verified as `W\target\debug\aleph-server.exe`, cmdline `aleph-server.exe --config W/.analysis/browser-live-runtime/config.toml start`, exe sha256 `1fe14217d3567b73edab1969a87a513012cf49f7554c81a4c97b304f9280a40f`. Only this PID was stopped; port 18790 confirmed free afterwards |
| Server build | `bash .analysis/browser-live-runtime/task51-gate.sh cargo build --offline --bin aleph-server` (same gate as the window-5 rebuild; sets `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_NET_OFFLINE=true`) → **exit 0**, 9m10s. Log `.analysis/browser-live-runtime/task56w7-build-server.log` (untracked) |
| New exe | sha256 **`8407d21aa621607d78233e4b847b5ca94455f696104ae4756180844391c44bb8`** |
| New Gateway | started by `.analysis/browser-live-runtime/task56w7-start.ps1` (copy of the window-5 start script: same exe, args, cwd, `ALEPH_HOME=W/.analysis/browser-live-runtime/home`, hidden/detached). New PID **2700**, same image path and cmdline, listening 127.0.0.1:18790, `/healthz` 200 after ~30 s. stderr has `Session migration failed: … no such table: sessions`, which is also in the window-5 log (pre-existing) |
| New RPC present | `.analysis/browser-live-runtime/task56w7-probe.js` (unauthenticated connect, no token): `browser.viewer.resource.list` → `-32008 denied`; a bogus `browser.viewer.no_such_method_x` → `-32601 Method not found`. So the method is registered |
| Mock LLM | PID 2664 on 127.0.0.1:18791 alive; not restarted |
| dist build | `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_NET_OFFLINE=true just wasm` → **exit 0**, `wasm-release` 8m02s; wasm-opt fence applied, webview baseline consistent, 41/41 wasm imports resolve, 4 files precompressed. Log `task56w7-wasm.log` (untracked) |
| dist before (window 6) | `aleph_panel_bg.wasm` `907049b6…9887c`, `aleph_panel.js` `587d10c1…de692` |
| dist after | `aleph_panel_bg.wasm` **`f53f3ff7a4d9c01e5a8460370c75e13f919ecf3372304088c6cac0e17d1f7d85`**, `aleph_panel.js` `223c7e4148320e1c23f001044a3a456ba3158e9c17adcc89b525d341ca3173b5`, `index.html` `f6dc82e5…7dbaf` (unchanged), `tailwind.css` `7bec2d98fee29b05fcd5915c78fedbb0deb8522d2fd8121aa0ec0107cd0d2ede`. List: `task56w7-dist.sha` |
| Served == dist | all four `GET /<file>` from PID 2700 sha256-identical to the dist files |
| Diagnostic strings | new wasm contains `browser.viewer.resource.list` and the console fragment `browser-live seed: ` (from `interfaces/webchat/src/app.rs`, outcome kinds applied/unchanged/ambiguous/stale/rejected/error) |

## 2. Harness change / 装置改动

- `qa/native_ipc/real_qa_prep.mjs:37`: `MAX_WINDOWS = 7` with comment "+ exactly one user-approved seventh window (2026-10-13, after seed/connected_since fix)".
- Self-tests updated the same way as window 6: index 7 with six 180000 ms priors accepted, index 7 with five priors refused, index 8 refused (`window_index_out_of_range`); pid-record and run-validation refusal cases moved 7→8. No phase criterion changed.
- `real_qa_prep.mjs --self-test` 34/34; `panel_driver.mjs` 37/37; `run-real.mjs --self-test` 15/15 + fixes-selftest 45/45 (pure fakes, includes the `connected_since` edge cases).
- Shell / reaper sha256 unchanged: `8d434299…49e0` (mtime 10-12 19:37) / `fcadd138…ec4ec`. No `desktop/shell` source newer than the shell exe; harness freshness list all older. Residual: `shared/protocol/src/devices.rs` / `browser_live.rs` are newer than the shell exe (shell depends on `aleph-protocol`), but `desktop/shell/src` uses neither `DeviceInfo`, `browser_live` nor `connected_since`; shell not rebuilt.
- Preflight (no `--execute`, index 7, `--confirm-owned-pid 2700`) → exit 4 `PREFLIGHT_OK_NOT_EXECUTED`; seam / exeFresh / reaperFresh / panelFresh / gatewayIdentity / fixtureUp all true.

## 3. The run / 运行（exactly once, 2026-10-13 11:28:37–11:29:00 +08:00）

`node qa/native_ipc/run-real.mjs --exe W/target/debug/aleph-desktop-shell.exe --exe-sha256 8d4342…49e0 --reaper W/target/debug/examples/qa_credential_reaper.exe --reaper-sha256 fcadd1…ec4ec --gateway-url ws://127.0.0.1:18790/ws --confirm-owned-pid 2700 --window-index 7 --window-ms 180000 --prior-window-ms 180000,180000,180000,180000,180000,180000 --execute` → **exit 1**, `status=FAIL`, `failedReason=dom_not_as_expected`, `cleanupState=CLEAN`, `documents=1`. Fresh random namespace `aleph-qa.device-credential.<random>` (removed, see §6). Raw output: `.analysis/browser-live-runtime/task56w7-run.json` (untracked).

| Phase | Result |
|-------|--------|
| preflight | ok |
| root_created / precheck_absent | ok (markers written) / target absent |
| shell_spawned | ok, `cmdlineOwned=true` |
| monitor_hello | ok, authenticated, originExact |
| empty_load | ok, `credentialPresent=false` |
| resource_created | ok, `agentRunOk`, `resourceSeen` (fixture), revision 1 |
| **reveal_browser** | **FAIL `dom_not_as_expected`**: `ack=false`, `resourceVisible=false`, `pairingNotice=false` |
| pair_clicked … server_same_device | **not reached** |
| monitor_streams | ok, 1 document, allComplete, anomalies 0, nativeTrace complete (1 record, 0 unconsumed), 1 refused connection (origin kind `appShell`) |
| cleanup_process / gateway / credential / root | ok (`ownedCmdlineProven`, orphans 0) / ok (`deviceRevoked`, `ticketRevoked`, `fixtureClosed`, `leftoverTicketsClean`, `noNewDevicesLeft`) / ok (`deleteExactOk`, `postAbsent`) / ok (`removed=true`) |

Device-edge fields (`baselineSince`, `connectionReplaced`, `sawDisconnect`, `reconnectedNewer`, …): **none produced** — `server_same_device` did not run. Post-reload DOM: **none** — no reload happened.

## 4. Subscription / seed evidence / 订阅与 seed 证据

Gateway log `.analysis/browser-live-runtime/home/logs/aleph-server.log.2026-10-13` from line 1224 (slice saved as `task56w7-gateway-log-slice.txt`, untracked):

| Connection (role inferred from timing) | Evidence |
|---|---|
| `:59206` Panel doc 1, unpaired | opened 11:28:49.081; subscribed `alerts.**` → `approval.**` → `config.**` → `browser.live.resource` → `browser.live.closed` → `runtime.agents.changed` → `canvas.updated` by 49.181, **before** `agent.run` started (49.276) |
| `:62871` fixture | subscribed `browser.live.*` 49.161; closed 57.388 |
| agent run | started 49.276, BrowserOpen approved, Chromium launched 50.212, run completed successfully 50.730; the fixture observed the resource (revision 1) |

- So the window-5 symptom (missing subscriptions) did **not** recur: the Panel subscribed `browser.live.resource` before the resource was published, exactly as in window 6.
- **No `browser.viewer.resource.list` call appears in the log.** This is expected for this window, not evidence of a bug: the seed only runs for a paired caller (`ViewerCaller::tracked` returns `None` unpaired) and the window never paired. Also unverified whether the handler logs at all. Panel console (`browser-live seed: …`, `events.subscribe …`) is **not captured** by the harness; no console line is claimed.
- The Gateway log has no per-delivery line, so whether the `browser.live.resource` frame actually reached `:59206` is **not proven**.

## 5. Where it failed and why (evidence, root cause NOT proven) / 失败点

`reveal_browser` acks false when the layout toggle or the enabled `workspace-tab-browser` button is absent (`qa/native_ipc/monitor_contract.mjs:550`). In `interfaces/webchat/src/components/workspace_panel.rs` the Browser tab exists only while `BrowserWorkspace::has_browser()` is true, i.e. the Panel model holds an accepted resource. Together with `resourceVisible=false`, the Panel model most likely never accepted the published resource in doc 1 — but which link failed is unknown:

1. frame not delivered to `:59206` despite the subscription (no delivery log), or
2. delivered but rejected/ignored by `apply_browser_event` / the model (no console capture), or
3. a Panel-side change since window 6 (the dist changed: `app.rs`, `bridge.rs` seed wiring; `layout_toggle.rs`, `token_wall.rs`, `workspace_panel.rs` are also dirty vs HEAD) altered the unpaired path or the toggle.

Window 6 passed this phase with the window-6 dist and old Gateway; window 7 changed both, so a regression in either cannot be excluded. One run; nothing verified beyond the above. Criterion not relaxed.

## 6. Cleanup verification / 清理核验（independent, after the run）

- No `aleph-desktop-shell` / `qa_credential_reaper` process.
- `cmdkey /list`: 0 `aleph-qa` entries; 0 `aleph-shell-qa-*` dirs under `%TEMP%`.
- New Gateway PID 2700 still running, `/healthz` 200; mock LLM PID 2664 still running (both left running as instructed).
- Chromium PID 6136 (launched by the agent's BrowserOpen inside the Gateway, not by the harness) is still running; not harness-owned, left untouched.
- `C:/Users/zou/.aleph` exists as a directory (existence check only).

## 7. Not done / 未做

- No eighth window, no retry, no Native/desktop-shell Cargo tests, no shell rebuild, no HOME/USERPROFILE change.
- `connected_since` device-edge witness and post-reconnect seed **not validated in a real window** (phases not reached).
- No Panel console capture; no root-cause analysis or fix for the `reveal_browser` failure; no offline reproduction.
- No whole-workspace tests/clippy (only the server build, `just wasm` and the qa/native_ipc self-tests ran). Nothing committed; no other docs edited.

## 8. Next input needed / 需要的决定

The authorized budget is used up. Suggested offline steps before any further window: (a) reproduce doc-1 unpaired reveal in the browser-live (non-Native) harness against the current Gateway + dist, with Panel console capture, to see whether the `browser.live.resource` frame arrives and what `apply_browser_event` returns; (b) diff the unpaired code path between the window-6 and window-7 dist (`app.rs` seed Effect, `workspace_panel.rs`, `layout_toggle.rs`); (c) only then, if wanted, explicitly authorize another window.
