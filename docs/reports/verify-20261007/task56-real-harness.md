# Task #56 — real Native harness (I/O layer + entry point)

> **Historical note / 历史说明：** the "UNRUN" status below is the state when this layer was first written. Real windows 1-3 were later run, and one user-approved fourth window (`MAX_WINDOWS` 3→4) failed overall with `dom_not_as_expected` while proving the server-side same-device edge; see `task56-native-real-window-3.md` and `task56-native-real-window-4.md`; a final user-approved fifth window (`MAX_WINDOWS` 4→5) failed at `reveal_browser`, see `task56-native-real-window-5.md`. Still PARTIAL, not production ready. / 下文 “UNRUN” 为本层首次写成时的状态；之后已运行真实窗口 1-3，并经用户批准运行第 4 窗口（整体 `dom_not_as_expected` 失败，但服务端同设备边沿已证明），见上述两份报告；仍为 PARTIAL，not production ready。

**Status: UNRUN.** The I/O layer and entry point are written and syntax/self-test clean, but the real
window was **never executed**: no shell launched, no keyring touched, no Gateway started/stopped, no
browser. A real run is currently expected to be **refused by preflight** (see prerequisites).

## Files (all new, none of the protected files edited)

| File | Role |
|------|------|
| `qa/native_ipc/real_run_io.mjs` | Real `deps` for `runWindow` (preflight, root, reaper, process, monitor WS server, Gateway client). Node stdlib + existing `ws`. Also hosts the fix self-tests (`node qa/native_ipc/real_run_io.mjs --self-test-fixes`, exported `selfTestFixes()`). |
| `qa/native_ipc/real_run_core.mjs` | Pure orchestrator / phase schema / verdicts (`runWindow`, `evaluateDeviceEdge`, `newDevices`, `finalize`). |
| `qa/native_ipc/run-real.mjs` | Entry. `--self-test` (pure fakes + loopback sockets) or real mode (`--execute` gated). |
| `docs/reports/verify-20261007/task56-real-harness.md` | This report. |

Untouched: desktop Rust, `monitor_contract.mjs`, `panel_driver.mjs`, `real_qa_prep.mjs`, `run-real.mjs`,
shared/client/reaper, all production source (`real_run_core.mjs` / `real_run_io.mjs` were revised by the audit fixes below). `real_qa_prep.mjs:42` `monitorNonceAuth` stays `false`.

## What the I/O layer does (each bullet is fail-closed; unknown => refuse/ambiguous)

- **Preflight (read-only)**: seam (`extractMonitorScript` of `desktop/shell/src/main.rs` + `evaluateSeam`
  against `monitor_contract` ACTIONS, selector ids in Panel source and in the served Panel `GET /` + assets);
  shell freshness (pinned sha256, `QA_BUILD_MARKER`, rendered-template bytes in exe, exe mtime vs shell sources);
  reaper freshness (sha, `--owned-root/delete-exact/check-absent` needles, mtime vs reaper sources);
  Gateway identity for the confirmed PID (CIM image == `target/debug/aleph-server.exe`, cmdline `--config`
  fixture `config.toml`, `127.0.0.1:18790` listener owner == PID, `GET /` 200); fixture page 200; fixture
  token file exists (size only, never read in preflight); memory gate `check_memory.py --required-bytes 4294967296`.
- **Root**: random `aleph-shell-qa-<hex>` direct child of realpath(os.tmpdir()); `.qa-owned` = nonce;
  `.aleph/.desktop-shell-panel-target` = `http://127.0.0.1:18790`. Remove only if dir name, parent and nonce still match.
- **Reaper**: re-hashes pinned exe each call; `check-absent` before spawn, `delete-exact` + `check-absent` in cleanup;
  stdout parsed to fixed `{ok,absent}` only.
- **Process**: re-hash then spawn with exactly `--qa-root --qa-nonce --qa-credential-backend os --qa-monitor-port`
  (via `shellArgv`), env = parent env + inline `TAURI_CONFIG` (exact `tauri.qa.conf.json` JSON, no ticket/token).
  Identity proof = CIM creation-time + command line containing root and nonce (`mayTerminate`). Descendants
  sampled every ~1.5s; terminate kills only recorded `{pid, created}` descendants + own child handle, then verifies gone.
- **Monitor server**: `ws` on `127.0.0.1` ephemeral port; path `/qa`; `Origin` exactly the Panel marker origin; Host check;
  one connection (second document refused and counted unless `expectReload`); first event must be hello with the root
  nonce; `StreamValidator` on every event; commands only via `buildCommand` envelope (`ask_pair`, `submit_ticket`,
  `reload`, `inspect`, `reveal_browser`). The ticket exists only in the `submit_ticket` envelope in memory.
- **Gateway**: JSON-RPC over `ws://127.0.0.1:18790/ws` with `Origin` = Panel origin; fixture device token read once
  in memory and dropped; `agent.run OPEN_FIXTURE`, `browser.live.resource` wait, `gateway.ticket.create`
  (ticket id recovered by `gateway.ticket.list` diff; ambiguity => `null` => stage fails, see S1), `gateway.devices.list`,
  `browser.viewer.resource.decide`, `browser.viewer.grant.issue` (`launch_url` discarded) / `revoke`,
  `gateway.devices.revoke` (refuses the fixture device id), `gateway.ticket.revoke`.
- **Output**: only the fixed-shape JSON of `real_run_core` (enums, booleans, bounded ints, hex digests).

## Independent-audit fixes (B1/B2/B3/S1/S2/S3)

- **B1 - device identity is the id, never the name.** `newDevices(devices, beforeIds)` (old `newPanelDevices` kept only as an
  alias) returns every listed id absent from the pre-pair set; `device_name` is display only and is not used as a filter.
  `device_paired` passes only if (a) the native trace (shell stdout pipe) recorded a `panel_credential_save` ok record (`saveConfirmed`; formerly the page monitor's `invoke_end`, superseded), (b)
  exactly ONE new id exists (`newDevices === 1`) and it is `connected`, and (c) the Panel DOM shows paired. Two or more
  new ids => `pair_not_confirmed_by_gateway`, nothing is revoked blindly and cleanup is reported AMBIGUOUS (exit 3).
  Metrics renamed: `newPanelDevices` -> `newDevices`.
- **B2 - "same device" needs a real edge.** `evaluateDeviceEdge` (pure): baseline `last_seen_at` must be nonzero and the
  device connected; a 100 ms sampler (started before the `reload` command, capped) must then observe the SAME id
  `connected=false` (old socket closed) followed by `connected=true` with `last_seen_at` STRICTLY greater than the
  baseline, with no re-save (`resaved=false`). Never disconnected / reconnect not newer / baseline 0 / sampler saw
  nothing => `unproven` => `device_same_unproven` (FAIL, never PASS). Id vanished or a second new device => `different`.
  Gateway `touch_device` stamps `last_seen_at` in ms on connect, so a strict edge is attainable on a real run; if a
  future Gateway rounds coarser the result is "unproven", not "pass". Phase fields: `baselineNonzero, sawDisconnect,
  reconnectedNewer, sameDevice, resaved`.
- **B3 - grant RPC is issuer-fixture evidence only.** `decide` / `grant.issue` / `grant.revoke` run with the FIXTURE
  token; the phase carries `evidence: "issuer_fixture_only"` (schema-enforced, checked again in `finalize`). Revoke uses
  `revokeGrantExact`: the result must be exactly `{revoked: true}` (the legacy lenient `revokeGrant` remains only for the
  older self-test fake). `PASS_WITHOUT_JPEG_FRAME` therefore means: Panel DOM protected persistence + exactly one new
  connected device (monitor-confirmed save, strict reload edge) + the fixture-issuer RPC contract. It does **not** mean
  a protected Panel action ran or any frame was delivered. A PASS now carries limitations
  `jpeg_frame_not_verified`, `protected_action_not_proven`, `grant_rpc_issuer_fixture_only`.
- **S1 - no ticket residual.** `mintTicket` records a baseline list, marks the creation ambiguous, creates, re-lists (retries
  within the budget) and diffs. If the diff is not exactly one id (or the create answer was lost / an added entry has no
  id) every newly added ticket is revoked (exact `revoked === true`, or proven absent on a fresh list) before returning
  `null`; unrevoked ids are retained. Cleanup calls `settleLeftoverTickets()` (re-diff + revoke + prove) and requires
  true (`leftoverTicketsClean`). A list/create failure that no later list can resolve stays ambiguous => exit 3. A gateway
  client without `settleLeftoverTickets` is treated as not clean (fail closed). Cleanup also re-lists devices and requires
  no new id left (`noNewDevicesLeft`).
- **S2 - global deadline.** The window deadline is set at `runWindow` entry (preflight time counts). Every wait gets
  `left(want)` = min(want, remaining); `waitDom` and sleeps are bounded the same way; stage boundaries re-check.
  Expiry => `window_timeout` then the normal cleanup. Note `MAX_WINDOW_MS` is 180 000 in `real_qa_prep`.
- **S3 - signals (limited, stated honestly).** No process kill from a signal handler. `installAbortFlag` (installed lazily at
  the first side effect, i.e. root creation) registers SIGINT/SIGBREAK handlers that ONLY set a flag; `runWindow` polls
  `deps.aborted()` at stage boundaries/waits, stops with reason `aborted` and runs the ordinary cleanup. Limits: an
  in-flight wait (up to ~30-60 s) delays the abort; SIGHUP / console-window close / `taskkill` / SIGKILL are NOT handled
  and leave cleanup undone (an operator must run the reaper `delete-exact`); a SECOND Ctrl+C abandons cleanup and exits 3.
  This is not claimed as a guarantee of cleanup on interruption.

## Exit codes
`0` PASS_WITHOUT_JPEG_FRAME · `1` FAIL · `2` invalid args · `3` cleanup ambiguous · `4` not run / refused /
preflight-ok-but-no-`--execute`.

## Verification actually run
- `node --check` on `real_run_core.mjs`, `real_run_io.mjs`, `run-real.mjs`: OK.
- `node qa/native_ipc/run-real.mjs --self-test` (re-run AFTER the fixes): `total 14, failed 0`, exit 0; no `aleph-shell-qa-*` / temp leftovers.
- `node qa/native_ipc/real_run_io.mjs --self-test-fixes` (new, pure: virtual clock, in-memory fakes, loopback fake Gateway):
  `total 25, failed 0`. Covers B1 (non-"Panel" name, two new ids, unacked save), B2 (edge table, never-disconnected,
  not-newer, baseline 0, re-save), B3 (label schema, exact revoke, non-exact revoke fails), S1 (multi-add revoke-all,
  stuck revoke retained, lost create, list failure ambiguous, id-less entry, single-add, orchestrator exit 3, missing settle),
  S2 (preflight counts against window, tiny window), S3 (flag-only handler, abort routes to normal cleanup).
  `run-real.mjs` is outside this change's scope, so the new suite is NOT yet wired into `run-real.mjs --self-test`;
  hookup = import `selfTestFixes` from `./real_run_io.mjs` and add its exit code to `selfTest()`.
  Covers: argv strictness; seam-missing / stale-exe / no-`--execute` refuse before any side effect; monitor never
  connecting is not a pass; unproven process stop => exit 3 and root kept; root layout/removal ownership;
  reaper argv/parse/hash pin; monitor Origin/nonce/path/second-document refusal and fixed command flow; Gateway client vs a
  local fake (secrets not returned, fixture device never revoked); process manager with fake CIM/spawn/kill;
  preflight fails closed. **No protected-success verdict is asserted from fakes.**

## Prerequisites before a real run (why it is UNRUN)
1. **Shell exe is stale.** `target/debug/aleph-desktop-shell.exe` (mtime 2026-10-12 16:25:51) is older than
   `desktop/shell/src/main.rs` (16:41:53, contains `reveal_browser` + workspace selectors) => preflight refuses
   `exe_older_than_sources`. Someone permitted to run Cargo must rebuild with the QA feature, then supply the new `--exe-sha256`.
2. Reaper exe is currently newer than its sources (OK), but its sha256 must be pinned by the operator via `--reaper-sha256`.
3. **Served Panel** at `http://127.0.0.1:18790/` must contain the `data-testid` selectors (token-wall, pairing-notice,
   browser-live-decision, workspace/layout-toggle/browser-tab). The served WASM must be rebuilt if it predates them.
4. Gateway PID 12704 (or current) must be the owned fixture Gateway (`--confirm-owned-pid`); fixture `:18791/page` up;
   memory gate >= 4 GiB free.
5. Fixture device must have owner/admin role (needed for `gateway.*` ticket/device methods).
6. Operator must pass `--execute`; the self-test and default mode never create anything.
7. `real_qa_prep` `PREREQUISITES` flags remain `false` until a real run proves them; this report does not flip them.

## Not done / not proven
- Real run never executed; the Gateway-side RPC sequence is validated only against a local fake and by reading handler source.
- **Real JPEG frame not verified** (`jpeg_frame_not_verified` limitation); no browser frame claim.
- Ticket is minted unbound (`gateway.ticket.create` without `user_id`): the redeeming Panel becomes owner; ticket id is
  recovered by list-diff. If another ticket appears concurrently the diff is ambiguous: every added ticket (including a
  foreign concurrent one) is revoked and the run fails closed.
- Grant RPC evidence is fixture-issuer only (`issuer_fixture_only`); no protected Panel action is proven
  (`protected_action_not_proven`).
- The strict reload edge depends on sampling at 100 ms; a disconnect shorter than that is "unproven" (FAIL), never a pass.
- CIM sampling every ~1.5s can miss very short-lived descendants; a mismatch yields exit 3, never a clean claim.
- Windows only (CIM/PowerShell); no cross-platform path.
- `monitorNonceAuth` is a nonce hello over an origin-checked loopback listener; not flipped in `real_qa_prep`.
- No Cargo, no commit; git state of the worktree otherwise untouched (new files untracked).
