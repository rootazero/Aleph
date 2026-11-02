# Panel native runtime completion plan
# Panel native runtime 完成计划

- Date / 日期: 2026-11-02
- Base / 基线: local `main` at `f6e18cdf8`; no push.
- Goal / 目标: Close the remaining native acceptance gaps for authenticated Panel project context, or produce a stronger source-backed BLOCKED result with every attempted boundary and cleanup fact recorded.
- Current result / 当前结果: The previous controlled Windows WebView2 run reached the real page and stopped at `protected_pairing: BLOCKED / pairing_surface_unavailable`. The runner returned `AMBIGUOUS` because the helper did not provide the required stopped facts. External CIM plus the feature-enabled exact reaper proved process and credential absence before removing the owned root. Native acceptance is still BLOCKED.
- Architecture / 架构: Keep Core as the only authentication, project-selector, bytes, trust, and revision authority. Keep the QA-only Gateway observer behind `native-qa`. Keep the Node runner responsible for owned lifecycle, evidence, deadlines, and cleanup. Keep the Tauri shell as the only native credential bridge. Do not add a second selector, a fake actor, an `actor=None` path, or a product test endpoint.
- Evidence rule / 证据规则: Native Rust trace proves native credential command execution; Gateway receipts prove the authenticated Core result; the page monitor proves only bounded DOM/lifecycle facts; provenance proves the exact artifacts used. No one evidence class substitutes for another.
- Global constraints / 全局约束: Do not touch the daily daemon, port `18790`, default release, production credential service, or old token-bearing connection files. Use a QA-owned root, `--no-default-features --features qa-owned-root`, `--qa-credential-backend os`, random disposable Gateway origin, exact `(service, account)` reaper operations, and recorded process identity. Never kill by process name or port. Before every Cargo command run `python qa/browser_live/check_memory.py` from the worktree root, use `CARGO_BUILD_JOBS=1`, and do not alter the Cargo environment through `bash -lc`.
- Launch gate / 启动闸门: The previous 120-second, one-launch budget is consumed. Source changes, tests, builds, provenance preparation, and read-only audits may proceed. A new WebView2 launch requires a new explicit approval with a concrete budget. Do not reuse the previous budget implicitly.
- Review focus / 审查重点: Pairing must follow a real product-supported path; same-device reload must load the OS credential; A/B/Global actions must reach the real Panel controls and authenticated Gateway; revocation must fail closed at the next boundary; endpoint/lease transition must use a supported product path or remain BLOCKED; cleanup must never remove an unproven root.

## Work order / 工作顺序

### 1. Reproduce and close the pairing-surface gap

Read and map the current path before changing it:

- `interfaces/webchat/src/context.rs`
- `interfaces/webchat/src/api/native_credentials.rs`
- `interfaces/webchat/src/components/token_wall.rs`
- `interfaces/webchat/src/platform/wide/views/browser/viewer.rs`
- `desktop/shell/src/main.rs` (`QA_MONITOR_JS`)
- `desktop/shell/src/connection.rs`
- `qa/native_ipc/native_project_native_io.mjs`
- `qa/native_ipc/native_project_gateway_smoke_test.mjs`

Write a failing source-backed test or diagnostic that distinguishes these states in the initial DOM:

- a real TokenWall with the fixed token input and submit controls;
- a real Browser pairing notice with the fixed pair-desktop control;
- neither surface, which must become `pairing_surface_unavailable`;
- a monitor-authenticated page that has loaded but has not reached the login wall.

Trace how the fresh shell obtains the Panel origin, how the bootstrap ticket reaches the Panel, and which existing production state causes TokenWall or PairingNotice to render. Use the current `community-live.mjs` fixture only where it represents the production Panel path. If the fixture cannot create a supported authentication surface without a new product endpoint or fake state, record the exact blocker in the plan ledger and stop that path. Do not add a hidden input, direct `invoke`, injected browser storage credential, synthetic DOM, or helper-side selection command.

Acceptance: the source explains one legitimate pairing route, or the run is explicitly BLOCKED with a reproducible reason. No WebView2 launch is part of this task.

### 2. Harden the runner lifecycle before any new launch

Correct and test `qa/native_ipc/native_project_native_io.mjs` and `qa/native_ipc/native_project_cleanup.mjs`:

- keep the helper listener and controlled stdin alive long enough to perform `revoke_exact` after the `stopped` message, then close stdin and prove child exit;
- make `helper.stop()` return the required stopped facts without falsely requiring child exit before the exact revoke step;
- make `helper.revokeExact()` prove the command result and final child exit;
- ensure `performExactCleanup` records the six steps in order and always derives the `cleanup` phase from the actual steps;
- keep root removal last and fail closed on any missing process, revoke, reaper, or absence proof;
- ensure `waitReceipt()` accepts only receipts with `seq > cursor` and records each receipt once;
- use one injected deadline for all launch, monitor, helper, receipt, reload, and project-action waits. The declared window must be a real wall-clock cutoff, not a range check.

Add lifecycle tests that use a real helper child where practical, plus deterministic fake tests for timeout, stale receipt, helper stop/revoke ordering, and root-last cleanup. A failed helper stop must leave later cleanup unexecuted and must produce `AMBIGUOUS`.

Acceptance: the runner can prove the cleanup contract without a native launch; no helper process or root is removed by an unproven shortcut.

### 3. Make artifact provenance independent and reproducible

Add a small QA-only provenance preparation path, preferably under `qa/native_ipc/`, rather than making the runner hash each artifact and call that a pin. It must record, before a launch:

- the exact shell executable digest;
- the exact QA Gateway helper digest;
- the exact feature-enabled `qa_credential_reaper` digest;
- the fresh Panel WASM digest and the source/build manifest digest;
- the Tauri QA config digest and the repository revision used for the run.

The runner must compare the recorded manifest against the files it is about to launch and fail closed on mismatch. Add a mutation test for each class of artifact. Keep the manifest redacted and free of credentials, paths that are not needed for evidence, and arbitrary text. Do not claim that a digest proves semantic freshness; it proves only that the recorded bytes are the bytes launched.

Acceptance: a stale or replaced shell/helper/reaper/WASM fails before WebView2 launch; a matching manifest is recorded as provenance.

### 4. Prove protected OS credential save, reload, and delete

After the pairing route is source-backed and the launch gate is explicitly approved, use the existing real shell and bridge:

1. Start the disposable helper and obtain a transient server-issued bootstrap ticket in memory.
2. Launch the QA shell with `--qa-credential-backend os` and the dynamic target marker.
3. Drive only the existing TokenWall or Browser pairing UI through the bounded monitor.
4. Require a native trace for `panel_credential_save` with the spawned shell PID and the run nonce.
5. Confirm the helper’s actual run-created device mapping only after the real connect receipt.
6. Reload the same WebView2 instance and require `panel_credential_load` with `credentialPresent:true`, the same Gateway endpoint, the same authenticated device identity, and a new authenticated connect receipt.
7. Stop the shell, run exact `delete-exact`, then exact `check-absent` using the feature-enabled reaper. Remove the root only after both succeed.

The trace must never contain the token or lease ID. The Gateway receipt must use the redacted device label and verified user from Core. A page monitor event alone cannot satisfy this phase.

Acceptance: save, same-device load, authenticated identity continuity, exact delete, and exact absence are all independently evidenced. Any missing item is NOT_RUN or BLOCKED.

### 5. Prove native Global/A/B project flow and revocation

With the protected credential established, use the existing `ProjectsSidebar` controls and fixed monitor actions:

- `reveal_projects`, then observe the authenticated Global discovery receipt;
- click the existing A control and require the Core observer’s `select` and `discovery` receipts plus an authenticated A asset receipt;
- click B and require the same Core revision, B discovery, B asset success, and A asset refusal;
- clear to Global through the existing Global control and require the Global receipt;
- revoke the exact run-created device through the helper, then perform the next supported discovery/asset boundary and require login-wall or refusal evidence from both the native trace/page monitor and Gateway receipt where applicable;
- keep delayed A fencing separate: a held A response arriving after B is useful evidence of the Gateway boundary, but native UI selected-project identity must be proven separately before claiming the native phase.

Do not use helper fixture commands to simulate selection. Do not treat a Node WS client as native project evidence. Do not claim native late-result or revocation PASS when the monitor cannot prove the required boundary.

Acceptance: all successful native phases have native trace, fixed DOM action, and authenticated Core receipt coverage; revocation is observed at the next real boundary; unsupported late-fencing evidence remains BLOCKED.

### 6. Resolve the endpoint/lease transition boundary

Audit the existing product-supported transition in:

- `desktop/shell/src/connection.rs`
- `desktop/shell/src/keyring_bridge.rs`
- `interfaces/webchat/src/platform/wide/views/settings/network/connection.rs`
- `desktop/shell/capabilities/default.json`
- `interfaces/webchat/src/context.rs`

Determine whether an existing user-visible path can change the endpoint while an authenticated connection is active and produce the required A→B→A lease fencing evidence. The runner may observe that path only through existing bounded UI and native trace. It may not edit the target marker during an in-flight connection, call `invoke` from the page monitor, or add a test-only endpoint.

If the current product has no supported bounded transition, leave the phase `BLOCKED` with the exact source reason and create a separate architecture proposal for any future UI/driver seam. This plan does not silently authorize product UI expansion.

Acceptance: either a real supported transition is evidenced, or the limitation is explicitly recorded as BLOCKED with no fabricated partial PASS.

### 7. Run the controlled native window and close the ledger

Before any new launch, produce a reviewable preflight report containing:

- explicit new launch approval and numeric window budget;
- owned root and nonce proof;
- shell/helper/reaper/WASM/config digests;
- disposable Gateway origin and helper PID;
- exact monitor and process ownership settings;
- expected evidence files and redaction rules;
- exact cleanup sequence and the condition for leaving the root behind.

Run at most the newly approved number of launches. Update:

- `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`
- `docs/reference/FEATURE_LOCATOR.md` §6.11.2

The ledger must distinguish `PASS`, `NOT_RUN`, `BLOCKED`, and cleanup `AMBIGUOUS`. It must state that the previous run reached WebView2 but stopped at `pairing_surface_unavailable`, and it must not retroactively upgrade that result.

### 8. Review and delivery

Run the full applicable Node suite, docs check, `git diff --check`, targeted `rustfmt --check`, default and `native-qa` Cargo checks, and the exact reaper build with `--features qa-service` when cleanup is exercised. Dispatch one fresh read-only whole-branch review after all code and docs are committed. Record timeout/SIGTERM explicitly; never call a non-clean review clean.

Deliver on a short branch based on the merged local `main`. Do not merge or push until the next session explicitly requests it.

## Suggested next-session validation set / 下一会话建议验证集

```text
python qa/browser_live/check_memory.py
node --test qa/native_ipc/*_test.mjs
python qa/browser_live/check_docs.py
git diff --check
rustfmt --edition 2021 --check <changed Rust files>
CARGO_BUILD_JOBS=1 cargo check --bin aleph-server
CARGO_BUILD_JOBS=1 cargo check --features native-qa --bin aleph-panel-native-qa-gateway
CARGO_BUILD_JOBS=1 cargo build -p aleph-client --example qa_credential_reaper --features qa-service
```

Do not run a WebView2 launch in the validation set until the next-session launch approval is explicit and the provenance/preflight report is complete.

## Definition of done / 完成定义

The next phase is **PASS** only if protected OS save/load/delete, same-device reload, native Global/A/B, authenticated revocation boundary, supported endpoint/lease transition, independent provenance, and exact cleanup are all evidenced. Otherwise the result remains **BLOCKED**, **NOT_RUN**, or **AMBIGUOUS** with the missing proof named precisely. A clean tracked branch and green tests do not convert a missing native runtime proof into PASS.
