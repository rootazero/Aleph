# Panel native authenticated project context acceptance design
# Panel native authenticated project context 验收设计

- Date / 日期: 2026-11-01
- Status / 状态: Draft — requires explicit approval before implementation / 草案——实施前必须获得明确批准
- Worktree / 工作树: `native-context-qa`
- Scope / 范围: QA-owned Windows Tauri/WebView2 acceptance only / 仅限 QA-owned Windows Tauri/WebView2 验收

## Intent / 意图

本阶段要证明已经完成的 Panel authenticated project context 能通过真实 Windows Tauri/WebView2 宿主抵达同一条生产 Gateway 的两条消费边界：RPC discovery 与 HTTP immutable asset。证明必须包含真实 native credential IPC、真实 OS protected persistence、Gateway 返回的 authenticated user/device 身份，以及项目选择、切换、延迟结果和撤销后的 fail-closed 行为。

This phase proves that the already implemented Panel authenticated project context reaches both production Gateway consumption boundaries through a real Windows Tauri/WebView2 host: RPC discovery and immutable HTTP assets. The proof must include real native credential IPC, real OS-protected persistence, Gateway-confirmed user/device identity, and fail-closed behavior for project selection, switching, delayed results, and revocation.

The existing product contract remains the authority. The acceptance harness must observe it through existing public behavior; it must not create a second selector, registry, bytes, trust, scope, revision, or authentication authority.

## Goals / 目标

1. Build and run the Panel-only QA shell as `--no-default-features --features qa-owned-root` with `--qa-credential-backend os`, using a proven owned root and a random disposable Gateway origin.
2. Pair one real device through the existing native handshake, save the server-issued device credential through the real native bridge, close/reload the WebView2 shell, and prove that the same OS-protected credential is loaded without re-pairing.
3. Prove that the authenticated Gateway actor reaches `panel.project.select`, discovery, and HTTP asset delivery. The Gateway receipt must identify the expected test user/device role without exposing a token.
4. Exercise Global, project A, project B, same-Core-revision switching, explicit clearing, cross-user refusal, workspace/membership/archive/device revocation, and next-boundary fail-closed behavior.
5. Exercise endpoint/lease changes and delayed A results so that an old credential/result cannot authorize, overwrite, or clear the current B state.
6. Produce independently reviewable evidence from native Rust trace, Gateway receipts, page lifecycle/DOM monitor, source/WASM provenance, process ownership, and exact cleanup.
7. Preserve the existing distinction between `PASS`, `NOT_RUN`, `BLOCKED`, and cleanup `AMBIGUOUS`.

## Non-goals / 非目标

- No change to Panel registry, immutable bytes, trust, scope, Core revision, Gateway authentication semantics, or project authorization rules.
- No durable process-global project selection, live notification, background refresh, or immediate idle teardown.
- No installation/update/uninstall, marketplace, signatures, release switching, external WASM, multi-file assets, Zahir runtime mount, or product test endpoint.
- No use of the daily daemon, its port `18790`, its process, its credentials, or its user profile.
- No claim that the old memory backend, JS VM driver, Rust-only tests, Chrome run, or page-side `invoke` callback proves protected native runtime behavior.

## Anti-goals / 失败方式

The result is a failure or non-result if it:

- uses the memory credential backend or a fake Gateway while claiming protected native success;
- passes a token, project root, device identity, or control secret through an argv string, ordinary log, screenshot, browser storage, or the forbidden token-bearing `connection.json` path;
- kills by process name or port, stops/replaces the daily daemon, enumerates credentials, or removes a non-run credential;
- treats a DOM monitor or screenshot as evidence of native credential execution;
- accepts `actor=None`, CWD, root, URL/query/header project inference, or a client boolean as project authorization;
- reports PASS when the Gateway identity receipt, native trace, served Panel provenance, or exact cleanup is missing.

## Constraints / 约束

- `desktop/shell` QA mode is debug-only and compile-time incompatible with `embedded-core`; use `--no-default-features --features qa-owned-root`.
- The shell must accept only a canonical temporary direct child named `aleph-shell-qa-*`, containing a matching `.qa-owned` nonce. The root must not be home, APPDATA, LOCALAPPDATA, an ancestor of those directories, a reparse path, or a path alias.
- The QA OS service is exactly `aleph-qa.device-credential.<nonce>` where the nonce is 32–128 lowercase hexadecimal characters. The account is derived from the canonical `ws://`/`wss://` endpoint. Production service `aleph-client.device-credential` is out of scope.
- `desktop/shell/src/qa_trace.rs` remains the authoritative child-side native evidence channel. Its fixed schema and contiguous sequence rules must be preserved.
- `qa/native_ipc/monitor_contract.mjs` page evidence remains limited to bounded lifecycle and DOM state. It cannot prove credential execution, identity, or authorization.
- Production Gateway behavior must use the existing `GatewayServer`, `DeviceTokenManager`, `ProjectStore`, Panel RPC handlers, and HTTP publication handlers. The helper may add only QA control and receipt plumbing outside the product authentication path.
- Every Cargo command starts with `python qa/browser_live/check_memory.py` from the worktree root, uses `CARGO_BUILD_JOBS=1`, and does not alter the toolchain through `bash -lc`.
- Any fresh Panel/WebAssembly artifact must be hash-pinned before the shell is launched. Served assets must come from the fresh embedded/production-library path, never an old `dist` directory or daily daemon.

## Approaches considered / 已比较方案

### Selected: independent native runner plus disposable Gateway helper / 选定：独立 native runner + disposable Gateway helper

The existing `qa/native_ipc/real_run_*` runner remains the orchestration shell for preflight, bounded windows, native trace parsing, process identity, and cleanup. A new QA-only Rust helper owns one in-process `GatewayServer` instance, isolated `ProjectStore`, test users/devices/projects, and an owner-only control channel. The helper binds HTTP+WS on `127.0.0.1:0`, reports only a redacted origin/receipt contract, and never writes a token-bearing connection file.

This keeps the production Gateway behavior close to `tests/panel_project_rpc.rs` and `tests/panel_http/live.rs`, while separating the native WebView2 run from the existing Chrome live fixture. The helper must be a real process owned by the runner so its PID, command line, root/nonce association, readiness, graceful stop, and exit can be proven independently.

### Rejected: extend the existing live fixture directly / 已否决：直接扩展现有 live fixture

The current `panel_asset_live_gateway_fixture()` is useful source material but its ignored `connection.json`, file commands, and fixture control lifecycle are unsuitable as native proof. Combining Chrome, WebView2, native credential operations, state mutation, and cleanup in that fixture would blur provenance and increase the chance of leaving token-bearing state behind.

### Rejected: Rust test as the complete GUI orchestrator / 已否决：由 Rust test 完整统筹 GUI

A Rust integration test could reuse fixture types directly, but it would own Cargo test lifecycle, a GUI Tauri child, WebView2, and OS keyring at once. Failure attribution and bounded cleanup would be harder to audit than a small helper process plus the existing Node runner.

## Design / 设计

### 1. Owned processes and data flow / 受控进程与数据流

The run has exactly three active owners:

1. **Node acceptance runner** — owns the run nonce, temporary root, helper child, shell child, monitor listener, evidence ledger, phase deadlines, and final cleanup.
2. **Rust Gateway helper** — owns one disposable `GatewayServer`, isolated project/security state, exact run-created device/ticket records, delay gates, and redacted Gateway receipts.
3. **QA Tauri shell** — owns the real WebView2 window and native bridge. It uses the existing `keyring_bridge` and `DeviceCredentialStore` path with the explicit QA OS backend.

The runner must communicate with the helper through a single owner-authenticated transient channel whose endpoint and authentication are bound to the run nonce and whose values are never placed in shell argv or ordinary logs. The protocol carries commands such as readiness, issue one-shot pairing input, select fixture state, hold/release a named delayed response, revoke an exact run-created identity, request a redacted receipt, and graceful stop. It must not expose raw device tokens, bootstrap tickets, project roots, credential payloads, or arbitrary JSON passthrough.

The initial one-shot pairing input must use the existing native handshake entry path. The implementation plan must identify the current supported handoff into the Panel without adding a product test endpoint; if the existing handoff cannot deliver the ticket without argv/log/file leakage, the run is `BLOCKED` and the harness is not weakened to force it.

### 2. Gateway fixture and receipts / Gateway fixture 与回执

The helper creates the same semantic fixture used by the production project acceptance:

- Alice and Bob users;
- separate paired device identities;
- project A and project B with controlled workspace roots and membership;
- a stable Core publication revision;
- production `panel.project.select`, snapshot/discovery, and HTTP asset handlers;
- controls for workspace unbind/rebind, membership removal, archive, device revoke, and a named delayed response.

Pairing uses the real bootstrap-ticket exchange and server-issued device token path. A receipt is emitted only after the actual Gateway handler observes the request. Receipts contain synthetic fixture labels such as `alice/device-a`, operation class, project label, revision, HTTP status or RPC outcome, and a monotonically increasing receipt sequence. They contain no raw token, bootstrap ticket, root, URL with secrets, or unrestricted request payload.

The helper must distinguish receipts for:

- native protected connect and same-device reconnect;
- Global, A, B, and explicit Global clear;
- wrong-user or cross-project refusal;
- discovery and asset results under the current selection;
- delayed A response released after B is current;
- endpoint/lease mismatch and post-revocation boundaries.

### 3. Native WebView2 phases / 真实 WebView2 阶段

The runner executes the following ordered phases, stopping with `NOT_RUN` or `BLOCKED` when a prerequisite is absent:

1. **Preflight**: verify worktree/source identity, fresh Panel/WASM hashes, QA shell/reaper identity, helper executable, owned-root rules, monitor port, no daily-daemon mutation, and memory gate.
2. **First pairing**: launch the helper and QA shell against its disposable origin, deliver one supported one-shot pairing input, and observe native trace showing `panel_credential_load` followed by successful protected save. The Gateway receipt must show the expected Alice/device-A identity and authorized response.
3. **Protected reload**: close the shell through its owned lifecycle, independently prove the child process tree ended, relaunch the same QA shell with the same owned root/nonce and endpoint, and require native load from the QA OS service without a new pairing ticket. The Gateway must report the same device identity, not a newly paired device.
4. **Global and A**: after the protected connection is current, verify Global discovery, select A through the real Panel path, verify Global+A discovery and an A asset response, and verify no root appears in page or Gateway receipts.
5. **B and same revision**: select B, verify the same Core revision with B replacing A, verify A asset refusal and B asset success, then explicitly clear to Global.
6. **Cross-user/refusal**: exercise Bob or an equivalent wrong-user project claim and require refusal without Global fallback. Compatibility credentialless behavior, if observed, may be recorded only as non-protected and cannot satisfy this phase.
7. **Delayed result fence**: hold the A response at the helper, make B current, release A, and verify that the late result cannot overwrite B, clear B, or authorize an A asset. The evidence must include the native/page attempt boundary and Gateway outcomes.
8. **Endpoint/lease change**: change the configured target through the existing connection/lease path or use a second disposable origin only if the product path already supports it. Verify stale A lease/credential operations fail closed and do not become evidence for the current target. If no product-supported target change can be driven without a new API, record this phase `BLOCKED` rather than adding a test seam.
9. **Project/device revocation**: apply workspace unbind, membership revoke, archive, and exact device revoke through the helper. Verify the next discovery/asset boundary fails closed, including the existing socket behavior, and that explicit Global fallback is allowed only where the product contract permits it.
10. **Evidence and cleanup**: close/reap the shell, stop the helper through its owned control path, revoke only run-created tickets/devices, delete exactly the QA `(service, account)`, independently verify OS-store absence, and remove the owned root only after all prior proofs pass.

### 4. Evidence model / 证据模型

The ledger stores four separate evidence classes:

- **Native trace**: parsed from the child process stdout, authenticated by the run nonce and child PID, with contiguous sequence and fixed command/result/error values. It proves command execution shape and credential presence only.
- **Gateway receipts**: emitted by the helper after real authenticated handlers run. They prove the expected user/device role, current project outcome, revision, RPC/HTTP result, and revocation boundary. They do not expose secrets.
- **Page monitor**: bounded WebView2 DOM/lifecycle observations from the existing monitor contract. It proves the real page loaded, reloaded, changed visible state, or observed an asset boundary; it cannot prove native auth.
- **Provenance/cleanup ledger**: source and WASM hashes, shell/helper identity, listener ownership, recorded PIDs/creation times/cmdline/root/nonce, exact credential namespace/account, ticket/device IDs represented only by internal run handles, and cleanup results.

A screenshot is optional supporting evidence. It cannot substitute for native trace or Gateway receipts. Any missing class downgrades the result from PASS.

### 5. Cleanup and failure semantics / 清理与失败语义

Cleanup is ordered and fail-closed:

1. stop the shell through its owned lifecycle and prove the recorded process tree is gone;
2. stop the helper through its authenticated control channel and prove its recorded process is gone;
3. revoke only exact tickets/device IDs created by this run;
4. delete exactly the QA service/account derived from this run’s nonce and endpoint;
5. independently verify the exact OS-store entry is absent;
6. remove the owned root only after all preceding proofs succeed.

The harness must never kill by process name or port, stop the daily daemon, enumerate credential entries, print `connection.json`, or delete an ambiguous credential. If any ownership, revocation, absence, or process proof is ambiguous, leave the root in place and report cleanup `AMBIGUOUS` with the reason. If a precondition is missing before mutation, report `NOT_RUN`; if a real product seam cannot provide actor-bound protected proof, report `BLOCKED`.

## Builder decisions left open / 留给实施计划的决定

- The exact Rust helper target location and build command, provided it links the production `GatewayServer`/handler path and remains QA-owned.
- The concrete Windows transient control transport, provided it is nonce-authenticated, owner-only, bounded, secret-free in logs/argv, and has a graceful-stop acknowledgement.
- The concrete existing Panel handoff used to deliver the one-shot bootstrap ticket without adding a product endpoint or weakening the browser/native boundary.
- The minimal fixture-control schema and receipt fields, subject to the fixed secrecy and provenance rules above.
- The exact WebView2 reload/relaunch driver action, provided both same-device identity and OS persistence are proved.

These are implementation choices only if they preserve the stated evidence and security contract. A builder may not resolve them by switching to memory storage, the daily daemon, a fake Gateway, a page-side identity claim, or a new product authorization seam.

## Builder-check dispositions / Builder check 处理结果

The fresh builder check returned five ranked questions before the child process ended with SIGTERM. The result is usable as a question pass, but it is not reported as a clean-exit review.

这次 fresh builder check 在子进程以 SIGTERM 结束前返回了五个排序问题。结果可作为问题审查使用，但不称为 clean-exit review。

- **Overall result / 总体结果 — answered by the acceptance requirement / 由验收要求回答**: Every required phase must be proven for an overall `PASS`. Any `BLOCKED`, `NOT_RUN`, or cleanup `AMBIGUOUS` result makes the native phase incomplete; an unsupported endpoint/lease phase cannot be silently waived.
- **Execution target / 执行目标 — builder call / 实施约定**: Target the current Windows host with the built debug `Aleph-panel-qa.exe` and its installed WebView2 runtime. The preflight records OS/architecture/runtime identity; this phase does not claim a cross-version Windows support matrix.
- **Evidence artifact contract / 证据产物约定 — builder call / 实施约定**: Write redacted, ignored evidence below `.superpowers/sdd/2026-11-01-panel-native-project-context-acceptance/`, with a machine-readable ledger, native trace, Gateway receipts, monitor report, provenance hashes, and cleanup result. Raw tokens, bootstrap tickets, credential payloads, and the existing `.panel-live/control/connection.json` must never be copied there.
- **Authorization matrix / 授权断言矩阵 — answered by the existing product contract / 由现有产品契约回答**: Wrong-user, invalid, cross-user, unbound, revoked, archived, and device-revoked claims fail closed at the next RPC/HTTP boundary. Invalid selected-project state does not fall back to Global. Explicit `project_id:null` restores Global only when the product contract permits it. A stale A result cannot overwrite or clear current B. These assertions must be represented as explicit ledger rows.
- **Credential persistence and deletion / credential 持久化与删除 — builder call / 实施约定**: After shell-side deletion, a separate exact verifier reads only the run-derived QA service/account through an approved OS-store path and requires `NoEntry`; it must not enumerate credentials or use a wildcard delete. If the verifier cannot prove absence, cleanup is `AMBIGUOUS`, the root remains, and the run is not PASS.

These dispositions resolve the builder-check questions without expanding the product scope. The implementation plan may choose the concrete executable and transport details, but it may not weaken any of these outcomes.

这些处理结果在不扩大产品范围的前提下解决了 builder-check 问题。实施计划可以选择具体 executable 和 transport 细节，但不得弱化上述任何结果。

## Acceptance / 批准门槛

This design is ready for implementation only after explicit user approval of this document. After approval, the next skill is `superpowers:writing-plans`; no product or QA implementation begins before that plan is written and accepted by the applicable workflow.

本设计只有在用户明确批准该具体文档后才可进入实施。批准后下一步是调用 `superpowers:writing-plans`；在计划完成并满足后续工作流前，不开始产品或 QA wiring。

## Explicitly not done / 明确未做

- No code, Cargo build, shell launch, Gateway launch, credential read/write/delete, browser automation, or cleanup mutation has been performed for this phase.
- The actual Windows WebView2 runtime, OS protected persistence, disposable Gateway ownership, actor-bound native proof, and exact cleanup remain unverified until an approved implementation is built and run.
- No merge, push, daily daemon interaction, production credential access, or old worktree/evidence deletion was performed.

- 本阶段尚未写代码、运行 Cargo、启动 shell/Gateway、读写删除 credential、自动化浏览器或执行清理变更。
- 实际 Windows WebView2 runtime、OS protected persistence、disposable Gateway ownership、actor-bound native proof 和 exact cleanup，仍须在设计批准并实施后验证。
- 未 merge、未 push、未触碰日常 daemon、未访问生产 credential，也未删除旧 worktree/evidence。
