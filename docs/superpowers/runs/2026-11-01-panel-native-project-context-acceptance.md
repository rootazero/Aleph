# Panel native authenticated project context acceptance run
# Panel native authenticated project context 验收运行记录

- Date / 日期: 2026-11-01
- Branch / 分支: `native-context-qa`
- Contract / 契约: `aleph-native-project-context-run-v1`
- Status / 状态: **BLOCKED — no native WebView2 acceptance was started**
- Result / 结果: The dedicated runner exited with code `5` and the redacted phase result `helper_ready=BLOCKED`, reason `project_selector_unavailable`.

## Scope / 范围

This run was intended to prove that the existing authenticated project selector reaches the same production Gateway through a real Windows Tauri/WebView2 shell, protected native credential persistence, discovery, and HTTP Panel bytes. The approved design required a truthful blocked result when the real UI path could not be driven.

本次运行原计划使用真实 Windows Tauri/WebView2、QA-owned-root lite shell、OS protected credential backend 和 disposable production-library Gateway，验证既有 authenticated project selector 同时到达 discovery 与 HTTP Panel bytes。批准的设计要求在真实 UI 路径无法可信驱动时返回阻断结果。

## Actual evidence / 实际证据

- `qa/native_ipc/native_project_run.mjs --repo-root . --execute` scanned the current `interfaces/webchat` source and found no project selector, `panel.project.select` UI call, or bounded project-selection monitor command. It returned `BLOCKED` before any helper, shell, root, WebView2, or credential mutation.
- The current QA monitor only exposes fixed actions `ask_pair`, `submit_ticket`, `reload`, `inspect`, and `reveal_browser`. It never calls Tauri `invoke` and has no project selector command.
- `panel.project.select` remains implemented and tested in Gateway/Core paths such as `src/gateway/handlers/panel_capabilities.rs` and `tests/panel_project_rpc.rs`; that Core/Gateway evidence does not establish a real native WebView caller.
- No `--qa-credential-backend os` shell run occurred. No protected persistence, same-device reload, endpoint/lease transition, project A/B selection, delayed A fencing, revocation boundary, native trace, page monitor, or cleanup proof was produced.

## Completed QA infrastructure / 已完成 QA 基础设施

- `6d1ccf38 test(native-qa): define project context runner contract`
  - Added the closed JSONL contract, redaction rules, phase/cleanup state machine, and protocol tests.
- `3fbc68999 test(native-qa): record helper asset receipts`
  - Added the QA-only `aleph-panel-native-qa-gateway` target and disposable random-port `GatewayServer` fixture with Alice/Bob/project state, bounded fixture controls, delay gates, and redacted receipt storage.
  - `python qa/browser_live/check_memory.py` returned `ALLOW`.
  - `CARGO_BUILD_JOBS=1 cargo build --features native-qa --bin aleph-panel-native-qa-gateway` succeeded. The only warning was the pre-existing `src/extension/projection.rs:140` `panel_snapshot` warning.
  - Protocol/helper smoke: 9 tests passed. The smoke covers random origin, transient pairing redaction, bounded fixture controls, delay gate acknowledgements, and graceful shutdown. It does not prove a real WebSocket/WebView2 or protected credential action.
- `ee328adb5 test(native-qa): enforce evidence and cleanup proofs`
  - Added redacted helper receipt/native trace/provenance aggregation and exact ordered cleanup tests. Node evidence tests passed.
- `a02a5f989 feat(native-qa): fail closed without project selector`
  - Added the dedicated runner preflight and a regression test proving the current source shape returns `BLOCKED` without creating owned runtime state.

## Validation / 验证

Passed:

- `node --test qa/native_ipc/native_project_protocol_test.mjs qa/native_ipc/native_project_evidence_test.mjs qa/native_ipc/native_project_run_test.mjs`: 16 passed.
- `node --test qa/native_ipc/native_project_protocol_test.mjs qa/native_ipc/native_project_gateway_smoke_test.mjs`: 9 passed.
- `python qa/browser_live/check_memory.py`: `ALLOW` before the helper Cargo build.
- `CARGO_BUILD_JOBS=1 cargo build --features native-qa --bin aleph-panel-native-qa-gateway`: passed.
- `git diff --check`: passed before the implementation commits.

The attempted Node `ws` experiment that tried to connect to the helper `/ws` and exercise a bootstrap ticket hung before the WebSocket `open` event and was withdrawn. It is not evidence of success and is not included in the pass count.

## Not run / 未运行

- Windows Tauri/WebView2 shell launch.
- `--no-default-features --features qa-owned-root --qa-credential-backend os` native shell acceptance.
- Protected OS credential save/load/delete or exact service/account absence proof.
- Real native trace and authenticated page monitor handshake.
- Production-library Gateway actor-bound project selection through the Panel UI.
- Project A/B/Global, delayed result fencing, endpoint/lease, workspace/membership/archive/device revocation phases.
- Full shell/WASM rebuild and served hash provenance for this native run.
- Whole-branch read-only review.
- Merge, push, or changes to the daily daemon, port `18790`, production credentials, or the old token-bearing `.panel-live/control/connection.json`.

## Required next decision / 后续阻塞

A future implementation can proceed only after a real, bounded project-selection path is identified in the product WebView or explicitly approved as a product change. The current runner must remain blocked. The helper control channel, direct `actor=None`, page-side fake `invoke`, browser storage, and synthetic selector receipts are invalid substitutes for authenticated native proof.

The worktree remains isolated and unmerged. No native PASS is claimed.
