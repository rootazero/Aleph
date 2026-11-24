# Panel native authenticated project context acceptance implementation plan
# Panel native authenticated project context 验收实施计划

- Date / 日期: 2026-11-01
- Goal / 目标: Execute the approved design in `docs/superpowers/specs/2026-11-01-panel-native-project-context-acceptance-design.md` and produce a truthful Windows native acceptance result for authenticated Panel project context.
- Architecture / 架构: A QA-only Rust disposable Gateway helper, a dedicated Node native acceptance runner, and the existing QA-owned Tauri/WebView2 shell. The helper owns production `GatewayServer` state; the runner owns lifecycle/evidence/cleanup; the shell remains the only native credential bridge.
- Tech Stack / 技术栈: Rust/Tokio/Axum and existing Aleph Gateway APIs; Node ESM; Tauri v2/WebView2; existing OS credential store and `qa_credential_reaper`; no new product endpoint and no new runtime dependency unless an existing workspace dependency cannot support the helper.
- Spec / 规范: `docs/superpowers/specs/2026-11-01-panel-native-project-context-acceptance-design.md`
- Global Constraints / 全局约束: Never touch the daily daemon or port `18790`; never read or print `.panel-live/control/connection.json`; use `--no-default-features --features qa-owned-root --qa-credential-backend os`; use an owned root and exact nonce/service/account cleanup; native trace, Gateway receipts, page monitor, and provenance remain separate evidence classes; any `BLOCKED`, `NOT_RUN`, or cleanup `AMBIGUOUS` prevents `PASS`; no `actor=None`, CWD/root inference, client boolean authorization, localStorage credential fallback, wildcard credential deletion, process-name/port kill, or product authorization API change. Before every Cargo command, run `python qa/browser_live/check_memory.py` from the worktree root; use `CARGO_BUILD_JOBS=1`; do not use `bash -lc` to alter the Cargo environment.
- Review Focus / 审查重点: Ensure the helper does not become a second project/bytes/trust authority; ensure raw bootstrap/device tokens never enter persisted evidence or ordinary logs; ensure helper and shell PIDs are owned and reaped by identity; ensure same-device persistence is proven by OS-backed reload/relaunch; ensure delayed A cannot mutate current B; ensure all failure labels are honest and cleanup leaves ambiguous roots intact.

## Implementation order / 实施顺序

Work through the tasks in order. Each task has one primary checkable outcome. Do not start a later task when the preceding RED or cleanup contract is unresolved.

### Task 1: Freeze the QA contract and add failing protocol tests

**Files / 文件**

- Add `qa/native_ipc/native_project_protocol.mjs`.
- Add `qa/native_ipc/native_project_protocol_test.mjs`.
- Add `qa/native_ipc/native_project_run_core.mjs` with only the initial contract constants and RED state-machine tests.
- Add `qa/native_ipc/native_project_run_io.mjs` with contract-level stubs only if needed by the tests.
- Leave `qa/native_ipc/real_run_core.mjs` and `qa/native_ipc/real_run_io.mjs` unchanged; they remain the old browser-live runner contract.

**RED / 失败先行**

Write Node tests that fail until the new contract exists:

- helper JSONL messages have fixed `ready`, `command`, `receipt`, `pairing`, `error`, and `stopped` shapes;
- raw `device_token`, bootstrap ticket, project root, credential payload, and arbitrary passthrough fields are rejected from persisted receipt/event objects;
- command names are closed and include readiness, issue-pairing, mutate-fixture, hold/release-delay, receipt query, revoke-exact, and graceful-stop;
- phase names cover protected connect, protected reload, Global, A, B, late A fencing, endpoint/lease, revocation, and exact cleanup;
- `BLOCKED`, `NOT_RUN`, and cleanup `AMBIGUOUS` cannot produce an overall PASS;
- the cleanup order is process tree → exact credential delete → exact absence verification → root removal;
- the runner cannot accept the daily daemon origin or fixed port `18790` as its helper origin.

**Implementation / 实现**

Define one redacted protocol shared by the helper and Node runner. The helper communicates over its child stdin/stdout pipes, not a token-bearing file or an HTTP product endpoint. The parent keeps any one-shot ticket in memory and forwards it only through the existing `submit_ticket` monitor command; the ticket is never written to the ledger. Define explicit redaction and validation functions so later runner code cannot accidentally persist an unvalidated helper message.

Define `REAL_CONTRACT = "aleph-native-project-context-run-v1"` in the new runner contract. Define stable phase/result enums and exit codes separately from the legacy `aleph-real-native-run-v1` contract.

**GREEN / 验证**

Run `node --test qa/native_ipc/native_project_protocol_test.mjs` and the new module self-test. This task is complete only when the tests pass and the old runner self-tests still pass unchanged.

**Commit / 提交**

`test(native-qa): define project context runner contract`

### Task 2: Add the QA-only disposable Gateway helper

**Files / 文件**

- Modify root `Cargo.toml` to add a QA-only feature and a `[[bin]]` target with `required-features`; do not make the helper part of default product builds.
- Add `src/bin/aleph-panel-native-qa-gateway/main.rs`.
- Add helper modules under `src/bin/aleph-panel-native-qa-gateway/` only when a module has an independent contract, for example `fixture.rs`, `protocol.rs`, and `receipts.rs`.
- Add Rust unit tests beside the helper for protocol validation and fixture state transitions.

**RED / 失败先行**

Add failing Rust tests for:

- random loopback binding never selects or probes `18790`;
- readiness exposes only a redacted origin and run identity;
- pairing creates a real bootstrap ticket for the requested fixture user and exchanges it through the existing `DeviceTokenManager` path;
- receipts expose fixture labels and outcomes but never token, ticket, root, URL query, or credential payload;
- Alice/Bob, A/B, workspace, membership, archive, device revoke, and delayed-response controls are deterministic;
- malformed commands, unknown handles, repeated exact revoke, and shutdown are fail-closed.

**Implementation / 实现**

Create a QA-only executable that:

1. accepts only non-secret run parameters such as nonce, fresh Panel shell path, and an optional bounded control configuration;
2. constructs an isolated `ProjectStore`, security store, users Alice/Bob, device identities, and projects A/B with the same workspace and membership semantics used by `tests/panel_project_rpc.rs`;
3. constructs `GatewayServer::with_config`/`GatewayServer::new`, installs the real `DeviceTokenManager`, security store, `connect` handler, Panel RPC handlers, and HTTP publication router;
4. binds HTTP+WS to `127.0.0.1:0` and serves the fresh Panel shell/assets through the helper’s existing fixture fallback mechanism, with no fallback to the daily daemon;
5. exchanges bootstrap tickets using the existing server-issued device credential path, never inventing an actor or calling `authz::project_for(..., actor=None)`;
6. implements only the closed owner-authenticated stdin/stdout control protocol from Task 1; controls mutate fixture state or release a named test gate, never authorization logic;
7. emits redacted Gateway receipts after real RPC/HTTP handlers observe the authenticated request, including fixture role, operation, project label, revision, result, and sequence;
8. supports a held A discovery/asset result so the runner can make B current before releasing A; and
9. exits only after an explicit graceful-stop command, reporting whether the listener and fixture-owned resources were closed.

Use existing production types and handlers rather than duplicating selector or publication rules. If the existing HTTP fallback cannot be reused from the binary without moving a non-test authority, keep the fallback limited to serving the already-built Panel bytes; do not move project authorization into the helper.

**GREEN / 验证**

From the worktree root run the memory gate, then `CARGO_BUILD_JOBS=1 cargo check --features native-qa --bin aleph-panel-native-qa-gateway`. Run the helper in a bounded self-test mode that binds a random port, exercises readiness/pairing/receipt/redaction, and exits without touching the shell or credential store. Run the focused Rust unit tests from the root package. Confirm with a listener/process check that no command contacted `18790`.

**Commit / 提交**

`test(native-qa): add disposable production gateway helper`

### Task 3: Connect the real shell monitor and exact credential verifier

**Files / 文件**

- Modify `qa/native_ipc/monitor_contract.mjs` only if runtime validation needs a missing fixed field; preserve the existing wire contract and ticket non-echo rule.
- Modify `qa/native_ipc/panel_driver.mjs` only for tests of any changed monitor contract; keep its scope explicitly JS VM only.
- Modify `qa/native_ipc/real_qa_prep.mjs` only to add the new runner/helper prerequisites and preserve old runner `NOT_RUN` behavior.
- Reuse `shared/client/examples/qa_credential_reaper.rs`; modify it only if the exact random-origin account marker cannot already be verified, and add a focused test if changed.
- Add `qa/native_ipc/native_project_evidence.mjs` for redaction, native trace, Gateway receipt, page monitor, and provenance aggregation.

**RED / 失败先行**

Add tests that fail until the wiring is complete:

- a monitor `hello.auth` must match the proven QA nonce before the run can treat page events as authenticated monitor evidence;
- malformed, replayed, gapped, oversized, or ticket-echoing page events remain rejected;
- `NativeTraceStream` remains authoritative for `panel_credential_*`; page events cannot satisfy that requirement;
- `credentialTarget(nonce, helperOrigin)` derives the exact QA service/account and refuses the production service or wildcard account;
- `qa_credential_reaper` output is accepted only as fixed redacted JSON and exact absence is required;
- evidence aggregation refuses PASS when any required evidence class is absent.

**Implementation / 实现**

Use the existing shell `--qa-monitor-port`, root nonce, `QA_MONITOR_JS`, `StreamValidator`, and `NativeTraceStream` seams. The runner must verify the monitor nonce handshake at runtime; do not create a second browser control channel. Reuse `createMonitorServer`, `createTraceHandle`, root/process identity helpers, and the existing reaper shape where possible.

The exact verifier invocation must be derived from the helper origin and current run nonce, use `--mode delete-exact` followed by `--mode check-absent`, and prove the same root marker and exact QA service/account. It must never enumerate the keyring or use production `aleph-client.device-credential`. A failed absence check is cleanup `AMBIGUOUS` and leaves the root.

**GREEN / 验证**

Run the monitor and evidence self-tests, the existing `panel_driver.mjs` self-test, `real_qa_prep.mjs --self-test`, and the reaper’s focused tests. Confirm that no test output contains a bootstrap ticket, device token, project root, or credential payload.

**Commit / 提交**

`test(native-qa): bind authenticated monitor and exact cleanup evidence`

### Task 4: Implement the dedicated native project runner and state machine

**Files / 文件**

- Add `qa/native_ipc/native_project_run.mjs` as the executable entrypoint.
- Complete `qa/native_ipc/native_project_run_core.mjs`.
- Complete `qa/native_ipc/native_project_run_io.mjs`.
- Add `qa/native_ipc/native_project_run_test.mjs` for pure state-machine and process/receipt tests.

**RED / 失败先行**

Add failing tests for the full bounded state machine:

- preflight is read-only and rejects daily daemon identity, stale shell/WASM hashes, missing QA marker, wrong feature mode, invalid root, fixed port, missing helper, missing reaper, and insufficient memory;
- helper is launched and identified by recorded PID, creation time, command line, root/nonce association, and owned listener;
- shell is launched with `--qa-root`, `--qa-nonce`, `--qa-credential-backend os`, and `--qa-monitor-port`, with no endpoint secret in argv;
- pairing, protected reconnect, and same-device identity are required before project phases can pass;
- phase receipts assert Global/A/B, same revision, A refusal under B, B success, explicit Global clear, wrong-user refusal, delayed A fencing, endpoint/lease refusal, workspace/membership/archive/device revoke, and next-boundary behavior;
- process cleanup, exact revoke, exact delete, exact absence, and root removal are ordered and idempotent only for the same run handles;
- timeout, SIGTERM, malformed helper output, monitor loss, missing native trace, missing Gateway receipt, and ambiguous PID/credential ownership produce `NOT_RUN`, `BLOCKED`, or `AMBIGUOUS` rather than PASS.

**Implementation / 实现**

Build the runner as a dedicated contract instead of changing the legacy browser-live runner. Import generic safe helpers from `real_run_io.mjs` only where their contracts are independent of the fixed daily daemon; do not import its fixed `GATEWAY_URL`, fixture token, grant phases, or old `REQUIRED_PHASES`.

The runner must:

1. create the run nonce and owned root using the existing root rules;
2. preflight source/served Panel, shell/reaper/helper hashes, Cargo feature identity, WebView2/OS information, monitor port, memory, listener ownership, and the absence of daily-daemon mutation;
3. launch the helper with piped stdin/stdout and wait for redacted readiness;
4. launch the QA shell against the helper origin and establish the monitor connection;
5. request one-shot Alice pairing through the helper and send the ticket through the existing `submit_ticket` command without persisting it;
6. parse native trace and Gateway receipts to prove protected Alice/device-A identity;
7. close/relaunch the shell with the same root/nonce and require stored OS credential load and the same device identity without re-pairing;
8. drive the project phases through the real Panel UI/RPC/HTTP paths, using helper gates and redacted receipts for controlled delay and revocation;
9. record each phase as a machine-readable ledger row with evidence references and no secrets; and
10. perform the exact cleanup order, leaving the root when any proof is ambiguous.

Use bounded per-phase deadlines and the approved window budget. The runner must not automatically retry a failed protected phase by re-pairing, create a second device to hide identity mismatch, or replace the helper after a failure.

**GREEN / 验证**

Run the new runner in preflight-only mode and require a truthful `NOT_RUN` when no approved execution is requested. Run its self-tests with fake in-memory protocol objects only for state-machine coverage; label them harness tests. Do not call those tests native PASS.

**Commit / 提交**

`feat(native-qa): orchestrate authenticated project context acceptance`

### Task 5: Build the real Windows QA artifacts and run the controlled acceptance

**Files / 文件**

- Use existing `desktop/shell` files and `desktop/shell/tauri.qa.conf.json`; change them only if a preflight-proven native seam is missing.
- Add the redacted run ledger at `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`.
- Store ignored machine artifacts under `.superpowers/sdd/2026-11-01-panel-native-project-context-acceptance/`.
- Update the approved design only for implementation deviations or evidence corrections; do not silently change the acceptance contract.

**RED / 失败先行**

Before executing the controlled run, verify that the runner refuses to proceed when any of these are false: fresh WASM and served hash match, QA shell marker and feature mode, helper binary identity, random helper origin, owned root proof, monitor nonce, OS backend flag, exact reaper, process ownership, and required memory.

**Implementation / 实现**

Build the fresh Panel/WASM and QA shell according to repository instructions, stage shell placeholders when required, and pin all source/binary hashes in the ledger. Run only the disposable helper and QA shell owned by this worktree/run. Exercise the approved phases once, capture native trace/Gateway/page/provenance evidence, and use exact cleanup. If Windows WebView2, protected store, actor-bound pairing, endpoint/lease control, or cleanup proof is unavailable, record `BLOCKED`/`NOT_RUN` and stop; do not substitute memory, fake Gateway, old daemon, browser storage, or page-side proof.

Run the existing production project test separately where useful as a regression reference, but label it Core/Gateway evidence rather than native evidence. Do not rerun the known monolithic Core lib-test configuration that causes LLVM OOM. Keep the Terminal public-assets test ordering constraint from the repository instructions.

**GREEN / 验证**

Run the narrow checks first: helper unit tests, Node protocol/runner self-tests, shell/native Rust focused tests, fresh production-library Panel test, then one controlled native run. Run `python qa/browser_live/check_docs.py`, `git diff --check`, and the required source/secret scans. Run a fresh read-only whole-branch review after the run. Report the review’s actual termination state, including SIGTERM/timeouts, and distinguish all skipped or unverified checks.

**Commit / 提交**

`test(native-qa): record Windows native project context acceptance`

## Final self-review / 最终自审

- **Spec coverage / 规范覆盖**: The plan covers disposable Gateway ownership, real OS credential persistence, same-device reload/relaunch, A/B/Global, delayed results, endpoint/lease changes, revocation, evidence separation, and exact cleanup.
- **Step granularity / 步骤粒度**: Each task has a single primary outcome and explicit RED/GREEN checks; helper, protocol, runner, runtime, and evidence are separated so failures remain attributable.
- **Type consistency / 类型一致性**: Rust owns Gateway and typed fixture state; Node owns orchestration and validated JSONL; the page monitor and native trace retain their existing fixed schemas.
- **Review focus / 审查重点**: The most dangerous regressions are second authority, secret leakage, actor substitution, stale-result acceptance, daily-daemon mutation, and unsafe cleanup; each is named in the global constraints or a task gate.
- **Proportion / 比例**: The implementation stays in QA and test infrastructure. Product authorization, credential semantics, Panel selector behavior, and runtime mounting are deliberately outside scope.

## Explicitly not done / 明确未做

This plan does not implement the helper, runner, shell wiring, build, Gateway launch, WebView2 launch, credential operation, or cleanup. It does not claim that the native phase is complete. It also does not authorize merge or push; the approved workflow remains isolated on `native-context-qa` until a separate user instruction.

本计划尚未实现 helper、runner、shell wiring，也未构建、启动 Gateway/WebView2、操作 credential 或执行 cleanup；它不代表 native 阶段已完成。计划也不授权 merge 或 push；经批准的工作流仍保持在 `native-context-qa` 隔离分支，除非用户另行明确要求。
