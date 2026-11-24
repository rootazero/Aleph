# Panel authenticated project context native acceptance completion
# Panel authenticated project context native 验收完成计划

> Status / 状态: preparation plan for the next session. Do not launch WebView2 while implementing or validating this plan. A launch is a separate gate and requires a fresh explicit user authorization after the preflight report is reviewable.
>
> 本文是下一会话的执行准备方案。实现和验证本方案期间不得启动 WebView2。只有 preflight 可审阅后，才能通过独立 gate 请求新的明确 launch 授权。

## Goal / 目标

Complete the native authenticated Panel project-context acceptance, or finish with a precise reproducible `BLOCKED`, `AMBIGUOUS`, or `NOT_RUN` result. The next run must never end as an unexplained generic `native_run_failed` with empty evidence.

目标是完成 authenticated Panel project-context native acceptance；如果仍不能完成，必须产出可复现且可定位的 `BLOCKED`、`AMBIGUOUS` 或 `NOT_RUN`。下一次运行不能再以证据全空的 generic `native_run_failed` 收尾。

The selector product repair is already on local `main` in `bb1bd7cf8` and is a separate product task. The native task must consume the current checkout and fresh artifacts, while preserving the existing native QA changes as their own work. No native claim may be inferred from the selector repair.

selector 产品修复已经在本地 `main` 的 `bb1bd7cf8` 中完成，属于独立任务。native 任务必须基于当前检出和 fresh artifacts 执行，同时保留现有 native QA 改动的独立边界；不能从 selector 修复推导任何 native 结论。

## Current facts / 当前事实

- Latest native run: `d044d278ae098998cb40c8c62166e23124be4f219a3b7d785f761a9fecccbfa9`.
- It reached `helper_ready: PASS` and `protected_pairing: PASS`, accepted one redacted `panel_credential_save` trace and one helper receipt, then stopped at `protected_reload / monitor_command / monitor_auth_failed`.
- All six cleanup facts were true: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, `owned_root_removed`.
- No accepted credential load/persistence, same-device reload, Global/A/B project, lease fencing, or revocation evidence exists.
- The old native provenance is stale after the selector commit. The next session must rebuild and independently prepare provenance for its actual HEAD.
- A concrete source mismatch is present: `real_run_core.mjs` calls `deps.monitor.expectReload()` before issuing the reload command, while `native_project_native_io.mjs` currently sends `monitorCommand("reload")` without first arming the monitor for a second document. `createMonitorServer` rejects a second document while one is connected unless `expectReload()` has been called. This must be fixed before any launch.

## Tech Stack / 技术栈

- Rust desktop shell and Panel WASM built with Cargo/Bun.
- Node ESM native QA runner under `qa/native_ipc`.
- WebView2 runtime driven only through the existing owned shell, helper, monitor, gateway, and exact reaper path.
- Authenticated credential effect evidence comes from the fixed Rust QA trace; JS evidence is a redacted lifecycle projection.
- No new dependency, RPC, wire field, browser storage, or product authentication path.

## Spec / 规格

The run is successful only if the evidence ledger accepts, in order, native save, helper receipt, reload armed before command, authenticated new document, native load with `credentialPresent:true`, and the required Global/A/B, lease/endpoint, revocation, and exact cleanup boundaries. If it blocks, the report must include the first failed phase/operation, fixed reason, last boundary, operation-scoped missing layers, fixed monitor/trace/helper/receipt diagnostics, and all cleanup facts. `native_run_failed` is allowed only as a final fallback for an unclassified failure and must never hide available diagnostics.

## Global Constraints / 全局约束

- Every Cargo command is preceded from the repository root by `python qa/browser_live/check_memory.py` and uses `CARGO_BUILD_JOBS=1`.
- No WebView2 launch before source changes, RED/GREEN tests, fresh artifacts, independent provenance, and reviewable preflight.
- At most one 120-second business window per explicit authorization; no relaunch without a new decision.
- Keep native QA changes separate from selector product changes and preserve unrelated worktree changes.
- Never record secrets, raw credentials, tokens, tickets, URLs, origins, ports, paths, request IDs, lease IDs, device IDs, or raw OS errors.
- Keep exact cleanup order and root-last semantics; cleanup never overwrites the primary business failure.

## Review Focus / 审查重点

- Reload arming happens before the second WebView2 document is created.
- Monitor diagnostics distinguish origin, concurrent/unarmed reload, channel, stream, timeout, auth, and close failures with fixed redacted reasons.
- Evidence scopes to the failed phase and operation; earlier save/receipt evidence cannot satisfy reload load.
- Rust trace shapes and selected-ledger sequence semantics remain compatible.
- Empty evidence arrays still produce a useful boundary report.
- Artifact/provenance checks are independent and non-tautological.
- Product authentication, UI, Tauri, and permission contracts remain unchanged.

## Architecture and evidence boundaries / 架构与证据边界

Keep the existing product and QA boundaries:

1. Projects/Global controls → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` → the authenticated production Gateway.
2. Credential effect proof comes only from the owned shell's authenticated `ALEPH_QA_TRACE` stream.
3. Helper receipts, monitor observations, DOM state, Node WS/HTTP smoke, VM fixtures, process cleanup, and provenance are separate evidence layers. None upgrades another layer.
4. The runner consumes an independently prepared provenance manifest. Execution never creates or rewrites that manifest.
5. Cleanup remains exactly six steps, in root-last order. Cleanup incompleteness or budget exhaustion retains its existing `AMBIGUOUS` semantics.
6. No product authentication semantic change, pairing UI, hidden selector, direct Tauri invoke, browser storage credential, fake actor, `actor=None`, test endpoint, daily daemon, port `18790`, wildcard reaper, process-name kill, or old token-bearing connection file.

保持既有产品和 QA 边界：

1. Projects/Global → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` → authenticated production Gateway。
2. credential effect 只能由 owned shell 的 authenticated `ALEPH_QA_TRACE` stream 证明。
3. helper receipt、monitor observation、DOM、Node WS/HTTP smoke、VM fixture、进程 cleanup 和 provenance 是独立层，不能相互升级。
4. runner 只消费独立准备的 provenance manifest，执行路径不能创建或改写 manifest。
5. cleanup 仍严格六步、root-last；cleanup 不完整或预算耗尽继续按既有语义为 `AMBIGUOUS`。
6. 不改变产品认证语义，不新增 pairing UI、hidden selector、direct Tauri invoke、browser credential、fake actor、`actor=None`、test endpoint，不连接日常 daemon，不碰 `18790`，不 wildcard 删除 credential，不按进程名 kill，不读取旧 token-bearing connection file。

## Required source audit / 必须审计的源码

Before editing, read these files and record the current failure boundaries in the plan-scoped progress ledger:

- `qa/native_ipc/native_project_native_io.mjs`
- `qa/native_ipc/native_project_run_core.mjs`
- `qa/native_ipc/native_project_evidence.mjs`
- `qa/native_ipc/native_project_runner.mjs`
- `qa/native_ipc/native_project_run.mjs`
- `qa/native_ipc/real_run_io.mjs`
- `qa/native_ipc/real_run_core.mjs`
- `qa/native_ipc/monitor_contract.mjs`
- `desktop/shell/src/main.rs`
- `desktop/shell/src/qa_trace.rs`
- `desktop/shell/src/keyring_bridge.rs`
- `src/bin/aleph-panel-native-qa-gateway/main.rs`
- `interfaces/webchat/src/api/native_credentials.rs`

特别记录这些 boundary：artifact/provenance、owned root、helper ready、monitor accept、monitor auth, reload arm, inspect result, pairing surface, ticket submit, native trace stream, credential save/load/delete, helper ack, receipt cursor, project command, endpoint/lease, exact revoke, shell stop, helper stop, root removal。

## Implementation tasks / 实现任务

### Task 1 — Fix reload monitor arming before launch

Modify `qa/native_ipc/native_project_native_io.mjs` so the reload sequence is:

1. set `currentPhase = "protected_reload"`;
2. call the monitor's explicit reload-arm operation (`monitor.expectReload()` through the existing dependency surface);
3. record a fixed monitor lifecycle event such as `reload_arm / attempted` and `reload_arm / observed` or a fixed failure reason;
4. send the existing `reload` command on the authenticated old connection;
5. accept and authenticate the new document;
6. only then wait for the post-reload native load trace.

Do not add a new command or product route. Do not call `expectReload` after sending `reload`; the server must be armed before the browser creates the second document.

Add a deterministic regression test with a fake monitor that fails if `expectReload()` is called after `command("reload")`, and passes only when it is called first. Add a real monitor-contract test showing that an unarmed second connection is refused and an armed reload connection is accepted. Keep all observer fields closed and redacted.

### Task 2 — Expose fixed monitor diagnostics at a failed connection boundary

Extend the existing monitor dependency surface with a read-only fixed-shape diagnostic snapshot. It may contain only bounded counters/enums already maintained by `createMonitorServer`, for example:

- `upgrades`
- `accepted`
- `helloSeen`
- `refusedOrigin`
- `refusedOriginKinds` with only `missing`, `appShell`, `other`
- `refusedOther`
- `refusedConcurrent`
- `streamInvalid`
- `installFailed`
- fixed `installFailedReason`

Never expose URL, Origin, host, port, socket address, raw WebSocket error, token, nonce, ticket, or arbitrary error text. Do not dump the diagnostic object into raw stderr as an unbounded object; use the existing fixed diagnostic channel and validate its exact shape.

When monitor acceptance fails, preserve the last diagnostic snapshot in the evidence report and derive a fixed reason:

| Condition | Fixed reason |
| --- | --- |
| `refusedConcurrent > 0` while reload was not armed | `reload_not_armed` |
| origin-only refusal | `monitor_origin_refused` |
| other upgrade refusal | `monitor_channel_refused` |
| accepted socket but invalid first stream | `monitor_stream_invalid` |
| no accepted/hello connection before deadline | `monitor_connect_timeout` |
| hello arrived but auth/origin failed | `monitor_auth_failed` |
| accepted connection closed before required response | `monitor_connection_closed` |

If multiple conditions exist, use a deterministic priority and record the complete fixed counters. The primary business failure remains first-failure-wins; cleanup remains independent.

Add RED/GREEN tests for every mapping and for the case where all public evidence arrays are empty. The report must identify the last boundary and missing layer without using `native_run_failed` when a more precise reason is available.

### Task 3 — Make each boundary observable without changing evidence semantics

Audit every call in `native_project_native_io.mjs` that sends or waits for a helper, monitor, trace, or receipt operation. It must emit fixed lifecycle events before and after the operation, including:

- layer
- phase
- fixed operation
- status
- `cursorBefore`
- `cursorAfter`
- fixed reason or `null`

No command arguments, request IDs, ticket values, device IDs, lease IDs, paths, URLs, raw errors, or secrets may enter the event ledger.

Ensure these semantics remain exact:

- `helperMessages: []` means no accepted redacted helper message was observed; it does not imply command success.
- `receipts: []` means no accepted receipt after the cursor; it does not imply the operation did not run.
- `nativeTrace: []` means no accepted selected credential effect was stored; the trace stream diagnostic must distinguish absent, poisoned, closed, and invalid ingestion.
- Wire trace sequence must remain contiguous in `NativeTraceStream`; the selected evidence ledger may skip wire records and only rejects duplicate/regressing selected sequence values.
- `primaryFailure` is first-failure-wins and cannot be overwritten by cleanup.
- `missingLayers` is scoped to the failed phase and operation.

Add tests for delayed, interleaved, closed, stale-cursor, duplicate, invalid, and all-empty evidence cases.

### Task 4 — Verify Rust trace and credential bridge contracts

Read and test the existing QA-only trace path:

- `panel_credential_save`: success has fixed `resultShape="null"`, `errorCode:null`, `credentialPresent:null`.
- `panel_credential_load`: success has fixed `resultShape="lease_object"`, boolean `credentialPresent`.
- `panel_credential_delete`: fixed success/error shape.
- Trace envelope authentication remains nonce/PID/sequence based.
- `keyring_bridge.rs` and `native_credentials.rs` retain fixed error codes and never expose backend text.

Only edit Rust if a deterministic parser or QA trace contract defect is proven. A Rust edit must be QA-gated, must not change production credential semantics, and must get targeted Rust tests plus the required memory-gated package check.

### Task 5 — Run all non-native RED/GREEN checks

Before any artifact rebuild or launch, run from the repository root:

```text
node --test qa/native_ipc/*_test.mjs
python qa/browser_live/check_docs.py
python qa/browser_live/check_docs.py --self-test
python qa/browser_live/check_memory.py
bun run check
cargo fmt --check
cargo clippy
```

The repository root currently has no `check` script, and the normal workspace hook has known baseline formatting issues. Do not silently call those commands PASS: record exact failure and continue with the package-scoped checks required by the actual change. For every Cargo command that is actually run, execute the memory gate immediately beforehand and set `CARGO_BUILD_JOBS=1`; never use `bash -lc` to hide environment setup.

Required focused checks:

```text
node --test qa/native_ipc/native_project_native_io_test.mjs qa/native_ipc/native_project_runner_test.mjs qa/native_ipc/native_project_evidence_test.mjs
cd interfaces/webchat && CARGO_BUILD_JOBS=1 cargo test --lib --no-default-features --features panel-modular
cd shared/ui_logic && CARGO_BUILD_JOBS=1 cargo test --lib composer_dials
```

Run the Node suite from the repository root so gateway smoke resolves `target/debug/aleph-panel-native-qa-gateway.exe`; running it from `qa/native_ipc` creates a known relative-path false failure.

### Task 6 — Rebuild current artifacts and prepare provenance

Only after source and test changes are complete:

1. Run the Panel live build from `D:/Workspace/Aleph/interfaces/webchat`:
   `bun run panel:tabs:build -- --clean --output=.panel-live/shell`.
2. Rebuild the native QA gateway, lite shell, and exact credential reaper with the required memory gate and `CARGO_BUILD_JOBS=1`.
3. Verify every artifact path and digest with fixed fields only.
4. Remove a stale generated provenance manifest only if it is the exact ignored manifest from this preparation and record why; never overwrite an existing manifest through the preparation command.
5. Generate a fresh manifest using exclusive creation and validate it independently against the exact current HEAD.
6. Record build ID, revision, artifact hashes, WASM hash, manifest hash, Tauri config hash, and validation result in ignored SDD evidence storage. Do not copy secrets or arbitrary paths into tracked docs.

Artifact acceptance requires:

- `build.json.revision === git rev-parse HEAD`;
- compiled terminal descriptor entry equals `build.json.entry`;
- expected host/terminal JS/WASM, HTML, CSS and manifest files exist;
- native shell/helper/reaper digests match the independent manifest;
- manifest revision equals current HEAD;
- no artifact check is tautological.

### Task 7 — Reviewable preflight and launch gate

Run the source-only runner without `--execute` and save its JSON output in ignored SDD evidence storage. It must show:

- `outcome: "NOT_RUN"`
- `reason: "preflight_only"`
- `webviewLaunches: 0`
- `launchGate: "new_explicit_budget_required"`
- `artifacts.ok: true`
- the exact six cleanup steps in order
- no native evidence falsely claimed

If artifacts are invalid, stop and fix the artifact/provenance issue. Do not launch to discover a setup failure.

At this point, present the reviewable preflight, the exact source changes, test output, and the planned one-window budget. The session must wait for a new explicit user authorization before running `--execute`.

### Task 8 — One bounded native window

After explicit authorization, run exactly one:

```text
node qa/native_ipc/native_project_runner.mjs --execute
```

Use the runner's 120-second business deadline. Do not start a second window in the same authorization. Do not run an ad-hoc shell, browser, helper, or reaper command outside the runner's ownership and cleanup path.

Expected evidence order:

1. artifact/provenance and owned-root identity;
2. helper ready;
3. monitor connection with authenticated hello;
4. pairing surface inspect with bounded polling;
5. existing pairing submission through the real UI path;
6. native `panel_credential_save` trace;
7. helper confirmation and receipt;
8. `monitor.expectReload()` armed before the reload command;
9. new monitor document authenticated;
10. native `panel_credential_load` with `credentialPresent:true`;
11. reload receipt;
12. Global discovery;
13. project A selection/discovery/asset;
14. project B same-revision selection/discovery and old-A fencing;
15. supported lease/endpoint boundary, or an explicit fixed `BLOCKED` reason if the current product path cannot drive it;
16. revocation boundary;
17. exact six-step cleanup.

### Task 9 — Evaluate without relaunching blindly

If the window reaches PASS for a phase, retain its evidence exactly. If it blocks:

- identify `primaryFailure.phase`, `operation`, `lastBoundary`, `failureReason`, `missingLayers`;
- include fixed monitor diagnostics, trace diagnostics, receipt cursor state, and helper lifecycle state;
- state which evidence arrays are empty and what that means;
- list all six cleanup facts independently;
- distinguish product/runtime failure from harness/ingestion failure;
- do not relaunch in the same authorization.

A second launch requires a new user decision and must be justified by a localized, evidence-backed defect. If the failure is `reload_not_armed`, `monitor_connect_timeout`, `monitor_origin_refused`, `monitor_stream_invalid`, `native_trace_missing`, or `receipt_timeout`, the next action must be a targeted fix and deterministic test before any new launch.

## Required deliverables / 必须交付

- Updated native I/O, monitor diagnostic, evidence, and runner tests with RED/GREEN evidence.
- A fixed boundary table in the plan-scoped SDD ledger.
- Fresh current-HEAD Panel/native artifacts and independently validated provenance.
- Reviewable source-only preflight with no launch claim.
- At most one explicitly authorized 120-second WebView2 run in the next authorization.
- Updated bilingual run ledger and `docs/reference/FEATURE_LOCATOR.md` §6.11.2 with exact native results.
- A final report that distinguishes product, Node/VM, helper, monitor, native trace, receipt, provenance, and cleanup layers.
- Conventional commit(s) containing only native plan-approved changes; no merge/push unless explicitly requested.

## Completion criteria / 完成标准

Complete only when either:

1. native save, helper receipt, same-device load with `credentialPresent:true`, Global/A/B project context, supported lease/endpoint boundary, revocation boundary, and exact cleanup are all proven; or
2. the run ends with a precise reproducible result naming the last boundary, fixed reason, missing layers, fixed diagnostics, cleanup facts, and the exact next architecture decision.

A generic `native_run_failed` with empty helper/receipt/trace evidence is never acceptable.

## Final report requirements / 最终报告要求

The final response must be Chinese and list:

- completed code and docs;
- exact commands and actual results;
- native run ID, phase outcomes, accepted evidence arrays;
- `primaryFailure`, `lastBoundary`, `failureReason`, `missingLayers`;
- fixed monitor/trace/helper/receipt diagnostics;
- all six cleanup facts;
- what remains unproven;
- review, format, and command limitations;
- whether another architecture decision or launch authorization is required.
