# Task #56 — sixth real Native window (index 6, after the Panel subscription fix): FAIL `device_same_unproven` at `server_same_device` / 第六次真实 Native 窗口：失败

**Verdict / 结论：overall FAIL (exit 1), cleanup CLEAN. Not a Native PASS; overall remains PARTIAL, not production ready.**
**整体 FAIL（退出码 1），清理干净；不是 Native PASS；整体仍为 PARTIAL，not production ready。**

Progress vs window 5 / 相对第五窗口的进展：window 5 failed at `reveal_browser`; window 6 passed `reveal_browser`, pairing, save, reload and stored-credential load, and failed two phases later at `server_same_device`. / 第五窗口卡在 `reveal_browser`；第六窗口通过了 reveal、配对、保存、重载、凭据读回，晚两个阶段才在 `server_same_device` 失败。

**Authorization / 授权：** user approved a dist rebuild + exactly ONE sixth window (fresh credential namespace, cleanup). `MAX_WINDOWS` 5→6. **The budget is used up: no seventh window, no retry.** Gateway was NOT restarted or rebuilt. `C:/Users/zou/.aleph` was only checked for existence. Nothing committed. / 预算已用完：不跑第七窗口、不重试。

## 1. Build / 构建

| Item | Result |
|------|--------|
| Cargo exclusivity | `tasklist` showed no cargo/rustc before the build; `py -B qa/browser_live/check_memory.py` → `available_bytes=10317303808 required_bytes=4294967296 ALLOW` |
| Command | `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_NET_OFFLINE=true just wasm` → **exit 0**, `wasm-release` 7m51s (total 9m22s); wasm-opt feature fence applied, webview baseline consistent, 41/41 wasm imports resolve, 4 files precompressed. Log: `.analysis/browser-live-runtime/task56w6-wasm.log` (untracked) |
| Sources in the build | `interfaces/webchat/src/context.rs` (mtime 10-13 09:16), `interfaces/webchat/src/app.rs` (10-13 09:06) |
| dist before (window 5) | `aleph_panel_bg.wasm` `ce0b0b68…374c1b`, `aleph_panel.js` `f6a53f7b…d6a1c` |
| dist after | `aleph_panel_bg.wasm` **`907049b6fc44279dffc22b72d0744d48fba9f31f80b4c9c964446b9557d9887c`**, `aleph_panel.js` `587d10c174922fad35908238f316e6f0ae135236cef83e2c2e86d140b34de692`, `index.html` `f6dc82e5…7dbaf` (unchanged), `tailwind.css` `7d53648d…783ae`. Full list: `.analysis/browser-live-runtime/task56w6-dist.sha` |
| Served == dist | `Cargo.toml`: `rust-embed = { version = "8.0", features = ["compression"] }` (no `debug-embed`, so debug builds read the folder from disk). Fetched from the running Gateway PID 1740 (no restart): all four `GET /<file>` sha256 **identical** to the new dist files. |
| Diagnostic string | the new wasm contains the `events.subscribe` console-format fragments (`events.subscribe …: …`, `events.subscribe …: error (replay)`), i.e. the new subscription code is in the served bundle |

## 2. Harness change / 装置改动

- `qa/native_ipc/real_qa_prep.mjs:37`: `MAX_WINDOWS = 6` with comment "+ exactly one user-approved sixth window (2026-10-13, after subscription fix)".
- Self-tests updated: budget test now accepts index 6 with five 180000 ms priors, refuses index 6 with four priors, refuses index 7 (`window_index_out_of_range`); pid-record and run-validation refusal cases moved from index 6 to 7. No phase criterion changed (`reveal_browser`, `server_same_device`, final acceptance untouched).
- `node qa/native_ipc/real_qa_prep.mjs --self-test` → 34/34; `node qa/native_ipc/run-real.mjs --self-test` → 15/15 + fixes-selftest 41/41 (pure fakes).
- Shell / reaper sha256 unchanged: `8d434299…49e0` / `fcadd138…ec4ec`. Preflight (no `--execute`, index 6) → exit 4 `PREFLIGHT_OK_NOT_EXECUTED`, seam / exeFresh / reaperFresh / panelFresh / gatewayIdentity / fixtureUp all true.

## 3. The run / 运行（exactly once）

`node qa/native_ipc/run-real.mjs --exe W/target/debug/aleph-desktop-shell.exe --exe-sha256 8d4342…49e0 --reaper W/target/debug/examples/qa_credential_reaper.exe --reaper-sha256 fcadd1…ec4ec --gateway-url ws://127.0.0.1:18790/ws --confirm-owned-pid 1740 --window-index 6 --window-ms 180000 --prior-window-ms 180000,180000,180000,180000,180000 --execute` → **exit 1**, `status=FAIL`, `failedReason=device_same_unproven`, `cleanupState=CLEAN`, `documents=2`. Fresh random namespace `aleph-qa.device-credential.<random>` (removed, see §5). Raw output: `.analysis/browser-live-runtime/task56w6-run.json` (untracked).

| Phase | Result |
|-------|--------|
| preflight | ok |
| root_created / precheck_absent | ok / target absent |
| shell_spawned | ok, `cmdlineOwned=true` |
| monitor_hello | ok, authenticated, originExact |
| empty_load | ok, `credentialPresent=false` |
| resource_created | ok, `agentRunOk`, `resourceSeen`, revision 1 |
| **reveal_browser** | **ok** — `ack=true`, `resourceVisible=true`, `pairingNotice=true` (failed in window 5) |
| pair_clicked | ok, tokenWall + tokenInput shown |
| ticket_submitted | ok, ticket minted, ack |
| save_ack | ok, save invoked, result null |
| device_paired | ok, gatewayConnected, newDevices=1, `uiPaired=true`, saveConfirmed |
| reload | ok, ack, newDocument, hello authenticated |
| stored_credential_load | ok, `credentialPresent=true`, `resavedOrRepaired=false`, `tokenWall=false` |
| **server_same_device** | **FAIL `device_same_unproven`**: `baselineNonzero=true`, `sawDisconnect=false`, `reconnectedNewer=false`, `anyAdvance=true`, `anyDisconnected=false`, `sampleCount=69`, errors 0, timeouts 0, `newDevices=1`, `resaved=false`; post-reload DOM: `domSeen=true`, `domResource=false`, `domPaired=false`, `domTokenWall=false`, `revealAck=false` after 3 attempts → `uiPaired=false` |
| monitor_streams | ok, 2 documents, allComplete, anomalies 0, nativeTrace complete (4 records, 1 unconsumed), 1 refused connection (origin kind `appShell`) |
| cleanup_process / gateway / credential / root | ok / ok (`deviceRevoked`, `fixtureClosed`, `leftoverTicketsClean`, `noNewDevicesLeft`; `ticketRevoked=false` with no leftover ticket) / ok (`deleteExactOk`, `postAbsent`) / ok (`removed=true`) |

## 4. Subscription evidence / 订阅证据

Panel console (`events.subscribe {topic}: ok|error(...)`) is **NOT captured** by the harness (it pipes only the shell's QA-trace stdout); no console line is claimed. Gateway log (`.analysis/browser-live-runtime/home/logs/aleph-server.log.2026-10-13`, from line 328; `handle_subscribe` logs only successful subscribes):

| Connection (role inferred from timing) | Subscribed (success lines) |
|---|---|
| `:54946` fixture | `browser.live.*` (09:33:19.953) |
| `:54945` Panel doc 1, pre-pairing | `alerts.**` → `approval.**` → `config.**` → `browser.live.resource` → `browser.live.closed` → `runtime.agents.changed` → `canvas.updated` … all by 09:33:19.960–20.050, **before** `agent.run` started (20.078); closed 23.379 (after ticket submit) |
| `:61873` Panel after pairing (replay order, alphabetical) | `alerts.**`, `approval.**`, `browser.live.closed`, `browser.live.resource`, `canvas.updated`, `config.**`, `runtime.agent…` (24.900–24.915); closed 25.032 with `CloseFrame 1001` (the reload) |
| `:61874` Panel after reload | same full set incl. `approval.**`, `config.**`, `browser.live.resource/closed` (25.586–25.673); closed 09:34:23.383 (os error 10054, shell stopped by cleanup) |

So the window-5 symptom (Panel never subscribing `approval.**` / `config.**` / `browser.live.*`) **did not recur**: all three Panel connections subscribed them, and doc 1 did so before the resource was published, which is consistent with `reveal_browser` now passing. This is correlation from one run, not a proof that the hardening was the cause.

## 5. Where it failed and why (evidence, root cause NOT proven) / 失败点

1. **Device edge not observed.** Gateway log shows the paired-device connection `:61873` closed at 25.032 and `:61874` opened at 25.568 — a ~536 ms disconnect gap. The sampler (one `devices()` RPC at a time, 100 ms sleep) took only 69 samples in ~53 s (~0.77 s per cycle), never saw `connected=false` (`anyDisconnected=false`) although `last_seen_at` advanced (`anyAdvance=true`). Hypothesis: the gap is shorter than the sampling cycle, so the strict "disconnect then newer reconnect" edge is missed. Not verified (no per-sample timestamps by design). Criterion not relaxed.
2. **Post-reload UI not paired.** After reload the DOM showed `resource=false`, `paired=false`, `tokenWall=false`; three reveals were not acked. `resource=false` matches the known gap (diagnosis §Proven 2: no replay/snapshot RPC, the in-memory Panel model loses a resource published before the reload). Why `paired=false` while `tokenWall=false` and the stored credential loaded was not investigated.

## 6. Cleanup verification / 清理核验（independent, after the run）

- No `aleph-desktop-shell` / `qa_credential_reaper` process; only `aleph-server.exe` PID 1740 remains (left running, `/healthz` 200).
- `cmdkey /list`: 0 `aleph-qa` entries; 0 `aleph-shell-qa-*` dirs under `%TEMP%` or the runtime dir.
- `C:/Users/zou/.aleph` exists as a directory (existence check only).

## 7. Not done / 未做

- No seventh window, no retry, no Gateway restart/rebuild, no Native Cargo tests, no HOME/USERPROFILE change.
- No Panel console capture; no root-cause analysis or fix for the device-edge sampling or the post-reload `paired=false`; no replay RPC.
- No JPEG frame, no protected Panel action, no production OS-keyring pairing, macOS/Linux not covered, managed companion BLOCKED.
- No whole-workspace tests/clippy (only the `just wasm` build and the qa/native_ipc self-tests ran in this task). Nothing committed; no other docs edited.

## 8. Next input needed / 需要的决定

The authorized window budget is used up. Options: (a) offline investigation of the device-edge sampler vs the ~0.5 s reconnect gap (e.g. use the Gateway's connection-close evidence or a faster/streamed device signal, without weakening the "same device" claim) and of post-reload `paired=false` / missing resource replay; then (b) if wanted, explicitly authorize another window.
